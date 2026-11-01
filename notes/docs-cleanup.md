# Docs cleanup (after the review and fixes)

Status: step 14a done 2026-10-31 (below); the owner's answers to "For the owner to confirm or drop" are left.

Owner, 2026-10-30: remove the old docs and Markdown files; keep only `notes/` and a rewritten `README.md`. Before
removing, extract what matters, especially the tasks the owner wants FERB tested on.

## In scope (61 tracked .md files outside `ferb/` and `ui/`)
- `docs/` (54 files): plan of record, decisions, launch-week specs, security test scenarios, owner acceptance log,
  superpowers plans/specs.
- Root: `ARCHITECTURE.md`, `CHANGELOG.md`, `AGENTS.md`, `CLAUDE.md`, `README.md`, `THIRD_PARTY_NOTICES.md`.
- `extensions/` (1 file).
- Non-Markdown docs to check at the same time: `docs/0.27_DECISIONS.toml` (read by `tests/test_decision_coverage.py`).

## Not in scope (stay)
- Runtime prompts are executable input, not docs: `ferb/orchestrator/prompts/*.md`, `ferb/memory/*.md`,
  `ferb/decide/prompts/*.md`, `ferb/abilities/**` (SKILL.md, README.md). They change only through notes/prompts.md.
- `ui/` (owner's side).

## Before deleting: extract into notes/
1. **`notes/test-tasks.md`**: every real task scenario the owner wants FERB tested on: live acceptance checks
   (`docs/OWNER_ACCEPTANCE.md`), documented security attack scenarios (`docs/0.27_SECURITY_TESTS.md`), live-test
   surfaces (FERB's Gmail and Discord, WhatsApp self-chat and self-only group, kartikey.xyz pages with the
   request-logging server, disposable files), the patch re-test list (notes/prompts.md decision 2) and the
   per-adapter live tests (notes/runtimes.md decision 1). Each with: the request, the surface, what pass looks like.
2. Owner decisions in the old docs that are still in force and not already in a notes file: list them with their
   source, for the owner to confirm or drop.
3. Anything else task-related the owner asked for (pending tasks, ideas such as the X bookmarks task).

## Decisions to raise at cleanup time
- `THIRD_PARTY_NOTICES.md` carries license attribution for bundled third-party code; removing it may break license
  terms. Keep it unless the owner decides otherwise after checking.
- `CLAUDE.md` and `AGENTS.md` are instructions for coding agents and point at the old docs; rewrite them to point at
  `notes/` (or remove), owner's call.
- `CHANGELOG.md`: release notes for users after launch; remove or restart.
- `tests/test_decision_coverage.py` depends on `docs/0.27_DECISIONS.toml` (the tests area decides its fate).
- CLAUDE.md was rewritten on 2026-10-30 to point at notes/ (owner: "it's stale"); the old conflicts are gone: rule 3 requires `tests/test_decision_coverage.py` and the
  decisions TOML; rule 21 and the development rules require ARCHITECTURE.md and CHANGELOG updates; rule 22 names the
  test-mode allowlist (kept in config, notes/gateway.md); rule 6 (agents decide retries) matches notes/orchestration.md.
- Old docs remain in git history either way.

## README rewrite
- What FERB is, install (the app), what it can do, permissions and why, privacy and security model in plain words,
  current limitations, development setup (checkout, tests), where the notes are.

## Step 14a (2026-10-31): done
- **Extracted** into `notes/test-tasks.md`: the live-test surfaces and how a run is judged; one step-13 checklist in 11
  sittings plus a "before step 13" block, each item tagged with its notes file; 11 target tasks (0.27 plan), 16
  acceptance tasks (A1–A16), 27 still-relevant live scenarios from the acceptance log (L1–L27), 40 security scenarios
  (S1–S38 from the catalogue, S39 delegated-conversation replays, S40 the P20 eval), 3 rule tests (T1–T3), the 33-item
  re-test list by reference to notes/tests.md (R1–R33), and 5 task ideas (X bookmarks first).
- **Deleted** (plain `rm`; all in git history at b6c3d120): `docs/` (54 Markdown files plus `0.27_DECISIONS.toml`, the
  baseline JSON, `diagrams/`, `evidence/`, `walkthrough/`), `ARCHITECTURE.md`, `extensions/store/privacy.html` (an
  older copy of `site/privacy.html`, which is the page the Web Store's privacy URL serves; nothing read the copy).
- **Readers fixed:** `tests/test_decision_coverage.py`, `tests/test_doc_sources.py` and the script it tested,
  `scripts/check_doc_sources.py`, deleted; `tests/test_chat_events.py::test_ack_phrases_are_the_plans_p25_list_without_the_starred_one`
  deleted (the P25 phrases stay in `ferb/orchestrator/acks.py`). No build script, the site or the extension read
  `docs/`. Comments that pointed at deleted files now point at notes or say "git history" (`ferb/core/interfaces.py`,
  `ferb/core/types.py`, `ferb/decide/decider.py`, `ferb/orchestrator/acks.py`, `tests/chromelab.py`,
  `tests/fake_extension.py`, `tests/test_browser_lab_real.py`, `tests/test_runtime_contract.py`,
  `tests/security/test_planner_injection_real.py`, `tests/support/injection_scenarios.py`).
- **Kept:** `CHANGELOG.md` (owner decides after launch: remove or restart), `THIRD_PARTY_NOTICES.md` (license
  attribution), `extensions/store/listing.md` with `store/src/` (the Chrome Web Store submission text and images, used
  for each extension upload).
- **CLAUDE.md**: only the line naming `docs/` and `ARCHITECTURE.md` changed (now names `CHANGELOG.md` alone). It no
  longer requires `tests/test_decision_coverage.py` (checked). **AGENTS.md** rewritten: read CLAUDE.md first, notes/ is
  the source of truth, plus the owner's preferences, branch, UI and build facts a non-Claude agent lacks.
- **CLAUDE.md, AGENTS.md and ui/AGENTS.md deleted** (owner, 2026-11-01: "everything imp is in notes now … lets keep in
  readme and notes only. i set these hard rules when i want"). Older citations of "CLAUDE.md rule N" in code comments,
  tests and notes point at the deleted file (git history has it).
- **README.md** rewritten from code and notes (permissions from `ferb/device/permissions.py`, retention from
  `ferb/store/retention.py` and `ferb/sense/perception.py`, runtimes from `ferb/runtimes/catalog.py`).

## For the owner to confirm or drop
Owner decisions found in the old docs that no notes file records (sources are in git history at b6c3d120). None was
moved into the area notes.
1. **License Apache-2.0** (docs/0.27_PLAN.md, launch-week clarifications, 2026-10-14). `LICENSE` and pyproject agree.
2. **Distribution** (0.27_PLAN.md L27, 2026-10-14; docs/superpowers/specs/2026-10-24-ferb-for-everyone-design.md): one
   FERB.app in a DMG, free signing only (no Developer ID or notarization for now), two DMGs (Apple silicon and Intel),
   updates install by themselves, the notch is the main interface and detail lives in a regular window; install from
   source stays supported.
3. **Download limits** (0.27_PLAN.md D35, 2026-10-09): 512 MB per download, 50 files and 4 GB per task, 50 new tabs per
   task, a 20 GB downloads folder whose oldest files go to the Trash only past it, a 10 GB free-disk floor, no
   time-based clearing; a browser download never opens a Save dialog. Settled 2026-10-31: notes/hands.md decision 7
   (no per-file cap, 300 files / 32 GB per task, shell downloads under the same budget and floor) replaces the
   per-download and per-task values and records the rest as unchanged.
4. **Clipboard and other agents** (0.27_PLAN.md D36, 2026-10-07: "no, let it do this"): FERB may write the clipboard and
   may approve another agent's prompts.
5. **Desktop changes are stated as facts, never blame** (0.27_PLAN.md D35/P26, 2026-10-09: "i dont want this hardcoded
   control over other agent's harness"): FERB records which app came forward, when its last on-screen action ended and
   which FERB-started processes ran, never who caused a change. notes/daemon.md decision 5 covers the sampler, not this
   rule.
6. **Screenshots** (0.27_PLAN.md L12, L13, D29; 2026-10-13/14, "no screenshots should be refused; go with redacted
   stuff"): the front window (topmost non-FERB window) is screenshotted, redacted, with every message and goes to the
   planner, the worker reuses it; browser windows, task tabs and recording frames go to the model redacted; an image is
   withheld only when masking fails; excluded sites are captured under `external:web:excluded`, excluded apps are not
   looked at or named; captures cover the app FERB works on, never the whole desktop.
7. **Typed text and clipboard** (0.27_PLAN.md D7, D11, D14, L17): typed text recorded per field with content;
   secure fields, Secure Input, password fields (shown ones too) and terminal apps never captured; no digit-shape OTP
   rule; typed and clipboard text kept 7 days. In code (`ferb/sense/perception.py` `purge_text`), but the About page's
   "What FERB keeps" table (`ferb/store/retention.py` RULES) has no row for it.
8. **FileVault and database encryption** (0.27_PLAN.md P8): setup/doctor checks FileVault; whole-database encryption
   once a signed helper exists. No notes file or code found for either (inferred from a search, not verified further).
9. **Questions never expire** (docs/superpowers/specs/2026-10-13-launch-week-fixes-design.md §8.1): a question stays
   open while its task waits; the runtimes' per-call tool timeouts stay at their defaults (2026-10-14).
10. **Big jobs start at once** (0.27_PLAN.md L3/D24, 2026-10-13): the plan line states the plan, estimates and any
    standing scope and work starts; the owner corrects by replying. Escape-hatch commands (D31) and retries after an
    unknown outcome (P19) still wait. Steering never records a No; an owner-driven retry re-asks a No once (ruling).
11. **Acknowledgement list** (0.27_PLAN.md P25 and L7): an instant acknowledgement from the daemon, from the approved
    phrase list without the starred one, never one of the last eight. The list now lives only in
    `ferb/orchestrator/acks.py` (its plan-reading test went with the plan).
12. **FERB doesn't recite its runtime, model names or design in chat** (0.27_PLAN.md P27 rule 8, 2026-10-08): dropped.
    Owner, 2026-10-31: notes/daemon.md decision 6 (About FERB) replaces it; FERB may discuss its models and design.
    No prompt carries the old rule (checked 2026-10-31).
13. **Loops** (docs/superpowers/specs/2026-10-24-loops-design.md, 2026-10-24/25): a loop with no stated end runs until
    the owner says stop, across restarts and updates; when fixes don't last it backs off and tells the owner once; the
    constants are the owner's (backup round 1,800 s; "didn't last" within 120 s; 3 such rounds in 600 s start back-off at
    120 s, doubling to 3,600 s; reset after 1,800 s); look-only rounds skip the check; held tabs are given back between
    rounds. In code (`ferb/orchestrator/loops.py`), not in notes.
14. **Failure reports** (docs/superpowers/specs/2026-10-24-failure-reports-design.md; 2026-10-25 address): the full
    record with secrets removed, previewed by the user; sent from the user's own Mail app to a dedicated feedback
    address (`ferb/daemon/reports.py`); offered after each failure. Owner, 2026-11-01: sent by FERB itself through the
    developer's relay to the private repo `Json604/ferb-reports` when the user presses Send; no Mail app, no address
    (notes/daemon.md decision 10).
15. **Code watchers** (docs/superpowers/specs/2026-10-25-code-watchers-design.md, 2026-10-25): long-running watches
    without AI calls (reload, read, view-switch clicks set up once), a 30 s floor, FERB looks up the service's rate
    limits and says which interval it chose. Decided as a design; not built.
16. **Proactivity** (docs/superpowers/specs/2026-10-24-proactivity-design.md, 2026-10-24): offer only (nothing done
    until **Do it**), Jev as a switch, no limit on approaches, a daily brief plus Mail and Calendar as sources. Not
    built; perception is paused until a Jev-free design (owner, 2026-10-27).
17. **Memory budgets**: settled. The owner's "Planner 60, worker 20" items (docs/superpowers/specs/2026-10-24-memory-that-works-design.md)
    were superseded by memory decision 7 ("retrieved within limits"); on 2026-10-31 the owner said that decision
    already covers the limits, so the per-role limits in notes/memory.md ("Limits per role") stand. Not to be re-asked.

Also for the owner: `extensions/store/listing.md` has stale lines (the repository "becomes public on 2026-10-21"; the
`ferb browsers pair` and `ferb setup --update` steps after approval predate the app's setup); refresh it before the next
extension upload, or drop it.
