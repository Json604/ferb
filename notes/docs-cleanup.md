# Docs cleanup (after the review and fixes)

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
