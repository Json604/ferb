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
- `test_decision_coverage.py`, `test_doc_sources.py` and one test in `test_chat_events.py` read docs/ (being deleted).
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
- CLAUDE.md rule 3 requires `tests/test_decision_coverage.py`; deleting it with the docs needs CLAUDE.md updated
  (owner question, notes/docs-cleanup.md).

## Fix list
- [x] Rewrite the conftest harness once for the new step kinds (no dispatcher) before deleting old harness tests.
      (Step 8a: `ROLES`/`DEFAULT_ANSWERS`/`role_of` in conftest; 8b changes those two tables when the checker absorbs
      the next-attempt choice. Step 8b: done; `report()`/`verdict()` helpers; scripts written before 8b are completed
      by `complete_worker_report` / `complete_checker_next`.)
- [ ] Write the 8-12 end-to-end runs; then delete the tests they replace.
      (Step 8a: tests/test_execution_runs.py: plain task, stop, pause/resume, plan step asks once, check-in Yes/No,
      scheduled declines and stops at 5, loop survives a restart, closed tab asks, taken tab, shutdown clean-up,
      resumed scheduled run. Remaining runs (file write checked, new recipient, unknown outcome, forget, payment) later.
      Step 8b: tests/test_merged_checker.py: fix_answer without a redo, a side detail marked inline, done-unconfirmed
      answer first, plain failure, carried conditions, failed attempts' artifacts, stale fields, a steer during the
      check, code facts, the how note, usage limit offers a runtime (question, RPC, none signed in, planner).)
- [ ] Keep/trim the safety files; move past-bug guards out of files being deleted.
- [ ] Add: import check, upgrade-from-old-DB, redaction vendor table + fuzz, simple forget, adapter contract tests.
- [ ] Delete tests of removed code and tests asserting prompt text, wording or wiring.
      (Step 3: tests of removed code deleted (see the step-3 report); prompt-text and wiring tests not yet.)
- [ ] Delete doc-reading tests with the docs cleanup.
- [ ] Keep `no_real_models`, `offline_trash`, network routing, `runaway.py`.
- [ ] Each area's fix lands with its tests in the same change (CLAUDE.md development rules).
