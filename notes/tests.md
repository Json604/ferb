# Tests: decisions and fix list

Area: `tests/` (287 test modules + 56 support modules, 92,230 lines), `tests/conftest.py`, `tests/support`,
`tests/security`. Basis: an AST/grep pass over every test file and `pytest --collect-only` (2026-10-30); nothing was
run. Product code is 64,703 lines.

## Owner decisions (2026-10-30)
1. **Small safety suite** (the target design below): safety floor and past-bug guards, 8-12 end-to-end runs with a
   fake model, import check, upgrade tests, adapter contract tests; no tests of exact wording, prompt text or wiring;
   live tests owner-run. Everything else goes. The end-to-end harness is rewritten before old tests are deleted.

## What exists today (verified)
- **4,070 test functions → 5,659 collected items** (parametrized); 5,510 run offline by default; ~142 live items
  (`real`, `real_claude`, `real_api`). Swift tests under ui/: 73 files (owner's side).
- Last measured full run: 13 min 46 s single-process for 4,569 tests (commit 89de579, 2026-10-25; its parallel-run
  change was reverted). Today probably ~16 min (estimate). Cost dominated by ~1,500 tests that start a real Daemon.
- Churn since 10-14: 1,117 test-file edits vs 1,572 product-file edits.
- By area: orchestrator 812 functions / 17.8k lines; gateway/policy/security 687 / 12.5k; memory/learning/forget 497 /
  10.1k; Mac 385 / 8.7k; browser 362 / 8.1k; daemon/RPC/setup/CLI 351 / 7.3k; sense/Jev 197 / 5.0k; shell/fs 224 /
  5.0k; runtimes 184 / 3.5k; store 160 / 3.1k; UI-adjacent 130 / 2.6k; prompts 50 / 1.2k.
- Kinds (heuristic, overlapping): exact prompt text 68-136 functions (+200 asserting context text sent to the fake
  model); owner-facing wording strings 960; internal wiring/mocks 897; through a public boundary 2,079 (566 already
  end-to-end through a real daemon + gateway with the scripted FakeRuntime); security/attack ~250-300 genuine.
- Overlap is conceptual: each review round added a file (6 question/answer files, 10 redaction files, 5 change-record
  files, 5 checker files, `*_026` and `test_rework_fixes` leftovers).
- **No import/compile check exists**; **no upgrade-from-old-DB test** (only fresh `migrate()`); no redaction fuzz or
  vendor-format table.
- `test_decision_coverage.py`, `test_doc_sources.py` and one test in `test_chat_events.py` read docs/ (deleted with the
  docs in step 14a).
- **The end-to-end harness** (`Script`/`FakeRuntime`/`install()`/`chat()` in conftest.py) hard-codes the `.dispatch`,
  `.verify`, `.question` steps (conftest.py:373-432); the dispatcher merge and supervisor rewrite break it and every
  test on it (566).
- Covering removed code (whole files, ~10.5k lines, ~587 items): learning/consolidation (10 files), forget echo scrub
  (3), Jev batching (3), CLI chat (3), steer (4), Codex exec, prompt contract, resource reviewer. Partial hits in
  surviving files ~467 functions. Total deleted anyway ~1,050 functions / 18-19k lines; ~1,000 more need rewriting if
  kept.
- **Tests guarding real past bugs** (must survive in some form): change-record symlink race leaking `~/.ssh`
  (test_changes_races, test_changes_protection); FERB's Mac tools pressing its own notch Approve button
  (test_own_apps); terminal typing routes and quoted secrets shown in the clear (test_terminal_input); owner's
  terminal typing captured (test_typed_text_capture); a typed "Yes" answering a payment (test_owner_answers I3 case);
  sink evasion by IP spelling and homoglyphs (test_sinks); TLS-off wrappers (test_tls_floor); sealed words in ids and
  one-time codes, ReDoS (redaction tests); developer-role prompt injection (security/test_planner_cache_injection);
  unknown-outcome retries (test_unknown_retry, test_ledger); trash without backup (test_recovery); sandbox rename above
  a protected path and planted links (test_shell_sandbox); Mac tools refusing the packaged interpreter (in
  test_front_window_context — move it).
- Guards that keep offline runs safe and must stay: `no_real_models` (no paid model calls), `offline_trash`, network
  routing, `runaway.py` (per-test timeout + memory cap, from the 110 GB hang of 10-25).

## Target design
A small suite that guards what live use can't show: the safety floor, plus a handful of end-to-end runs.
1. **Safety suite**, table-driven where possible: confirmations (pay, delete, new recipient, account security,
   foreground, browser pages, TLS-off, terminal typing, escape hatch), protected paths, sink rule and origins,
   redaction (one corpus + vendor-format table + fuzz), sandbox and proxy (real sandbox-exec), S1 backups, ledger/P19,
   own-app refusal, owner secrets, prompt-injection scenarios. Each past-bug guard above kept in some form.
2. **8-12 end-to-end runs** through the real daemon, gateway and a rewritten fake runtime: a plain task; a file write
   checked by the checker; a send to a new recipient asks and a No sends nothing; an unknown outcome is never retried
   without the owner; stop mid-task; pause/resume; a loop survives a restart; a forget removes the item; a payment
   asks once.
3. **Import/compile check** over every ferb module.
4. **Upgrade tests**: a fixture DB and config at an old schema with rows, upgraded to current, data kept (notes/
   storage.md decision 5).
5. **Runtime adapter contract tests** (~10) on the existing fake providers / fake Codex app-server.
6. **Live tests** stay owner-run (`-m real`), per area, including the per-adapter live tests and the patch re-test
   list.
7. No tests of exact prompt text, owner-facing wording, or internal wiring.
- Estimated: 40-50 files, 600-800 items, 9-12k lines including support; ~2-3 min single-process (estimate).

## Depends on
- notes/orchestration.md: the harness rewrite goes with the orchestration rewrite.
- notes/storage.md decision 5: upgrade tests on copies of real older databases and configs.
- The old CLAUDE.md rule 3 required `tests/test_decision_coverage.py`; the 2026-10-30 CLAUDE.md doesn't, and the test went
  with the docs in step 14a.

## Fix list
- [x] Rewrite the conftest harness once for the new step kinds (no dispatcher) before deleting old harness tests.
      (Step 8a: `ROLES`/`DEFAULT_ANSWERS`/`role_of` in conftest; 8b changes those two tables when the checker absorbs
      the next-attempt choice. Step 8b: done; `report()`/`verdict()` helpers; scripts written before 8b are completed
      by `complete_worker_report` / `complete_checker_next`.)
- [x] Write the 8-12 end-to-end runs; then delete the tests they replace.
      (Step 8a: tests/test_execution_runs.py: plain task, stop, pause/resume, plan step asks once, check-in Yes/No,
      scheduled declines and stops at 5, loop survives a restart, closed tab asks, taken tab, shutdown clean-up,
      resumed scheduled run. Remaining runs (file write checked, new recipient, unknown outcome, forget, payment) later.
      Step 8b: tests/test_merged_checker.py: fix_answer without a redo, a side detail marked inline, done-unconfirmed
      answer first, plain failure, carried conditions, failed attempts' artifacts, stale fields, a steer during the
      check, code facts, the how note, usage limit offers a runtime (question, RPC, none signed in, planner).
      Step 8c: tests/test_chat_turns.py: 4 quick messages are 2 turns, the time-limit reply with a restart, another
      chat's task stopped, a stop after finished work, folders in the plan line and shell scope, keep the tabs open,
      context order, long-chat bounds (planner and summary calls), question kinds, a fact's source after a clarifying
      question, a misrouted answer, a mid-task identity change said.)
      Step 8d: tests/test_answers_in_steps.py: one ask unchanged, a one-ask plan answers with its last step (no
      summarizing call), two parts fill out of order once each, a direct part in the first reply, a part asks while
      another fills, a failed part (2 of 3), a paused part, an updated row, a restart posts nothing twice, parts repair;
      tests/test_task_preview.py rewritten as behaviour guards.
      Step 8e: tests/test_typed_cards.py: note/event cards on a reply, a schedule's event card, no card for a refusal,
      receipt cards on a one-ask outcome and a multi-part first reply (and part counts), a scheduled run's reminder card
      with and without a next run, a payment from page facts (a refused one has none), a send from the checked
      destination, link and file cards; chat.status question fields, the FERB.app-only error code.
      Step 8e follow-up: tests/test_schedules.py: an end date from the owner's words stored and shown, no run after the
      end and the expiry line, end words the owner didn't type refused, set_end, a pre-0052 schedule unchanged (upgrade);
      tests/test_typed_cards.py: the countdown field on a run's card, a payment naming its saved card / none for a
      one-time hidden input, no card digits.)
      Step 9a (memory): tests/test_core_profile.py (the booking replacement and its line, same-key replace, the
      profile in every role's instructions, the hard budget, what joins at once and what waits to hold up, forget and
      retire leave it, the weekly pass, the memory page RPCs); tests/test_memory_capture.py rewritten for no Keep
      (FERB's understanding saved at once, decision-11 real chats through the chat path, the distill's chat pass, the
      distill never re-proposing a stored key, correction by talking, an unknown replaced id); tests/test_memory_upgrade.py
      (the clean-up and seed on a schema-52 store shaped like the owner's, idempotent on a second start);
      tests/test_setup_persona.py rewritten for decision 16 (observed, shown once, setup vs chat, refresh only on
      change, holds up before Tier 1); test_memory.py (pass verdict verified, forget leaves known destinations);
      test_memory_retirement.py (retire told, not asked; a due check). Keep/Edit/Drop and Retire/Keep tests deleted.
      Step 9b (memory): tests/test_memory_recall.py (one search finds facts, past tasks and old chat by meaning with the
      fake embedder and not by keywords alone; the planner's matching old exchanges, never one already shown;
      chat.history by meaning and date, paged from SQL; no fact copies and the old ones closed once; the planner's
      memory / old chat / activity within limits on a 10,000-fact store; how-notes from recorded calls, the worker and
      the owner's hints; playbook merge, a declined group, the repeat measure in the RPC and `ferb stats`; one card
      source per origin set; decision 17 by source with restore; decision 18's card and the cache guarantee; a burst of
      owner facts with one early pass a day); tests/test_perception_9b.py (accessibility out of activity and purged at
      7 days, browser privacy, values-only recent-first activity search, the activity limit, typed text across a focus
      change, the software watcher on change, the bot-check fact); tests/test_memory_embeddings.py rewritten for the
      static model (tokenizer, pooling, hashes, the fetch with a fake, int8 ranking); test_memory_search.py and
      test_worker_memory.py moved to memory_items and the role limits (the 60/20 and fact-node tests replaced).
      Step 9c (memory import): tests/test_memory_import.py (each reader on a fixture, redaction before any call, recall →
      verify, FERB's understanding / no destinations, newer wins, resume, cancel, nothing raw kept, the quality eval and its
      `-m real` variant); tests/support/import_fixtures.py.
      Step 10b (prompts): tests/test_prompt_assembly.py (a role block only for the run that needs it, no tool catalog in
      any role's context and no catalogs for the planner, the effect contract once in the worker's instructions and not
      on each tool); tests/test_owner_memory_seed.py (the approved items once as the owner's words, inferred duplicates
      ended, M7/M8 search only, the visa username's login record, a second start changes nothing, another store gets
      none).
      Step 12d: tests/test_safety_runs.py (a send to someone new asks and a No sends nothing; an action whose outcome is
      unknown is never repeated without the owner; one payment asks once with the page's amount). The two runs left
      already existed: a file write checked = tests/test_careful_checks.py, a forget removes the item =
      tests/test_forget_in_chat.py. Then 106 files the runs and the safety files make redundant were deleted (see
      "Step 12d: the deletion" below).
- [ ] Keep/trim the safety files; move past-bug guards out of files being deleted.
      (Step 12d: kept whole: every file of the target design's safety suite and every past-bug guard listed above (all
      in kept files; test_front_window_context is kept, so its packaged-interpreter guard stays where it is). Moved: the
      banner-secret test (test_app_notifications → test_app_screens). Left: trimming inside the kept files (their
      wording and wiring tests, e.g. most of test_question_clarity, test_front_window_context, test_install,
      test_setup_flow) — a per-test pass, not done.)
- [x] Add: import check, upgrade-from-old-DB, redaction vendor table + fuzz, simple forget, adapter contract tests.
      (Step 12a: upgrade-from-old-DB done: tests/test_upgrade.py builds stores as older builds left them (schemas 18–55,
      the pre-checksum runner, the deleted 0040 from git history) and an older config.toml, and upgrades them.)
      (Step 12d: import check tests/test_imports.py (every ferb module but `__main__` entry points, in a fresh
      interpreter with a temporary FERB home); vendor table + fuzz in tests/test_redaction_rebuild.py (the table, the
      idempotence/JSON/stream fuzz from 6a, and `test_a_vendor_secret_anywhere_in_ordinary_text_is_always_masked`, which
      found a leak: a truncated private key with words after its last line kept that line; fixed in
      ferb/sanitize/formats.py `_KEY_BODY`); simple forget tests/test_forget_in_chat.py; adapter contract
      tests/test_runtime_contract.py (every direct-API adapter), tests/test_codex_streaming.py (Codex app-server pinned
      fixtures), tests/test_claude_steering.py (Agent SDK), tests/test_structured_results.py,
      tests/test_provider_streaming.py.)
- [ ] Delete tests of removed code and tests asserting prompt text, wording or wiring.
      (Step 3: tests of removed code deleted (see the step-3 report); prompt-text and wiring tests not yet.)
      (Step 12d: deleted with the 106 files: Jev/consolidation/eval-catalogue tests, the prompt-text, wording and
      rendering files, and the old-harness wiring files; the two prompt-text tests left in test_owner_answers removed.
      Left: wording and wiring tests inside the kept files (see the trim item above).)
- [x] Delete doc-reading tests with the docs cleanup.
      (Step 14a: deleted tests/test_decision_coverage.py, tests/test_doc_sources.py with its scripts/check_doc_sources.py,
      and tests/test_chat_events.py::test_ack_phrases_are_the_plans_p25_list_without_the_starred_one; nothing else read
      the deleted docs. The P25 phrases stay in ferb/orchestrator/acks.py.)
- [x] Keep `no_real_models`, `offline_trash`, network routing, `runaway.py`.
      (Step 12d: kept in tests/conftest.py (`no_real_models`, `offline_trash`, `network_route`, `fake_browser_peer`,
      `offline_tokenizer`) and tests/support/runaway.py with its tests, tests/test_runaway_guards.py.)
- [x] Each area's fix lands with its tests in the same change (CLAUDE.md development rules).
      (A standing rule, kept by every step; step 12d's fixes each came with theirs.)

## Step 12d: the deletion
Before: 300 test files, 92,866 lines with support, 5,545 offline items (5,544 passed, 1 skipped) in 20 min 01 s
(measured while other work ran on this Mac). Deleted 109 test files (listed below; three with the eval code they
tested) and 8 orphaned support modules (batches_reference, bus, codex_loopback, context_reference, interleave, loop_fakes, model_loopback, signing_stubs).
Kept: every target-design safety file, every listed past-bug guard, the step 8a-12c behaviour files, the upgrade,
import and adapter-contract tests, the guards, the live (`-m real`) tests for step 13, and the step-14 doc tests (deleted in
step 14a). After:
see notes/fix-order.md step 12d. Deleted, by reason:
- removed or switched-off code (Jev, consolidation, the eval catalogue) and memory internals (12): test_decide,
  test_decide_real, test_destructive_shadow, test_release_measurement, test_security_harness, test_batcher_cost,
  test_attention_understanding, test_perception_batches, test_learning_calls_visible, test_learning_after_forget,
  test_memory_activation, test_memory_lineage
- prompt text, owner-facing wording, rendering (21): test_prompt_order, test_full_answers, test_answers_owner_words,
  test_answers, test_failure_reasons, test_notices_in_chat, test_connection_notice, test_progress_line,
  test_trace_render, test_trace_levels, test_cli, test_unit1_speed_voice, test_history_cap, test_banners,
  test_app_notifications, test_codex_prompt_cache, test_claude_prompt_cache, test_model_request, test_conditions,
  test_orchestrated_control, test_completion_reporting
- orchestration wiring and old-harness behaviour now covered by the end-to-end runs (40): test_supervisor,
  test_statuses, test_report_check, test_task_plans, test_control_turn_entries, test_steering_races, test_live_steering,
  test_tools_loaded, test_checker_effort, test_runtime_selection, test_api_runtime, test_codex_isolation,
  test_usage_prices, test_input_char_cap, test_context_budget, test_context_sources_026, test_hardening_context,
  test_literal_observation_026, test_outcome_export_026, test_evidence_inventory_026, test_daemon_shutdown_026,
  test_acceptance_one_worker, test_named_target, test_after_action, test_launched_apps, test_task_status_facts,
  test_task_recovery, test_inspection_recovery, test_delegated_conversations, test_loops, test_watches,
  test_waits_and_loop, test_ux, test_planner_context, test_episodes, test_content_search, test_recall, test_bus,
  test_clock, test_f6_context
- hands features with no safety check (browser, Mac, extension plumbing) (27): test_availability, test_background_input,
  test_browser_background, test_browser_default, test_browser_hardening_026, test_browser_media, test_browser_recovery,
  test_capture_errors, test_closed_shadow_frames, test_cursor, test_desktop_attribution, test_device,
  test_extension_autoinstall, test_extension_install, test_front_tab_script, test_human_check_handoff, test_mac_pointer,
  test_mailbox, test_named_browser, test_native_hardening, test_pair_all, test_pointer, test_scroll_progress,
  test_shell_attach, test_window_input, test_perception_retention, test_perception_software
- app and setup plumbing (9): test_appcast, test_update_packaging, test_update_prepare, test_login_item_uninstall,
  test_permissions_on_first_need, test_setup_rpcs, test_tokenizer_cache, test_verdict, test_window_rpcs

## Live re-test list (step 13)

Also in the one step-13 checklist, notes/test-tasks.md (sitting 9, scenarios R1–R33).

Step 10b removed the prompt patches (notes/prompt-patches.md) and rewrote the prompts to contracts; these live runs
show whether a removed line mattered (prompt-patches.md "Re-test", deduplicated, plus prompts.md's prompt-only
behaviours). Each runs on owner-controlled surfaces (CLAUDE.md rule 16); a run that sends to a real third party only in
an owner-approved run.

Patch re-tests:
1. "who's this actor?" on a show frame: the reply says it's a likely match and from what, or checks the cast (R4).
2. "what is this song?" on an Instagram reel; "hear and tell" on a video (R4, audio_listen).
3. An assignment done in full, FMI-style, with no caveats and nothing saying AI (M1).
4. "how are you better than dots?": FERB described from its installed capabilities (R4).
5. A username given mid-task, then a later task logs in with it (login_origin, M2).
6. Kaggle via Codex Astra: Codex does the work, the owner controls the browser; "score well, no overfitting" are
   conditions (R1, R5, M6, M9).
7. "storage is 80% again, use mole" with Mole not installed: one install question, never another tool (M3).
8. "open my scaler gmail, find the latest FMI assignment" (M4, R2, R3).
9. Scaler, 5 days, official groups only (R2, R3, M5, M7).
10. The 1-week study material (R1, R3, M8).
11. The visa job: a six-hourly check with no slots is a finished check (R8); pause, then resume, and its schedule
    follows (R7); the login with its three security questions asks nothing for the sign-in (effect contract); the check
    through the bot check (the ability); the task with Chrome New_Personal closed (browser open starts it).
12. A friend-style read-only "reduce my storage" scan plus a Homebrew cleanup: FERB's own snapshot is no change (R8).
13. "every 5 minutes …" is accepted as a schedule (no 15-minute floor).
14. "take mac to low power mode … loud warning at 10%": the task now plus a watch in the same turn (R9).
15. "every morning at 9 email me …": the address from memory ("email me").
16. "remind me about this show on oct30 anytime": FERB picks a time and says which (R10).
17. "do this for all the tabs in parallel": one step per tab, parallel never a condition (R1).
18. The hotspot question, then "leave it, but tell me why": continued without changes, not stopped (R7).
19. FMI Assignment 2 with "astra high": requirements read first, Codex configured (R3, R5, M6).
20. "check scaler mail for the new assignment": the current item by its dates (R3).
21. Concert tickets now (primary seller pages) and "analyze all coding agent sessions" (every source, no caps)
    (R1, R3).
22. Three ZIPs checked for name and student ID: only the intended replace, no box over PDF text (R6).
23. The Codex startup fix in VS Code: terminal typing asks first with the exact command.
24. The EF saved-application page with only a footer badge: not reported as a block (the bot-check fact's wording).
25. "keep it running, only stop when I say", then a follow-up: the earlier run's calls aren't this run's failure.
26. "did the PDF I uploaded come from Drive or my Mac?" (R2).
27. An X post from Brave Work (owner-approved run only): the campaign's standing scope.

Prompt-only behaviours (notes/prompts.md):
28. Pausing a task when the owner doubts or wants to rethink it, and resuming it.
29. Questions ask only what/who/which, never how or for permission.
30. A named tool that isn't installed is asked about once (as 7).
31. How-note quality on a verified task (the `how` field reads as steps that worked).
32. A direct reply about the screen names its source and says a guess as a guess (as 1).
33. Role blocks: a loop round arms a probe and keeps its tab; a watch run alerts once and then stays quiet; a delegated
    conversation turn waits and hands over.
