# CLAUDE.md — how to pick up FERB

FERB is a personal agent runtime for macOS. The owner's goal, in their words: "an agent that can control my Mac the
way I can, and do things building on top of that base, and keep learning with doing." Any app, any browser, worked
**in the background** (the owner keeps using the Mac), asking the owner only when a choice is genuinely theirs.

This file is for any agent or person starting cold. Nothing here depends on a previous session.

## Where things are

| Read this | For |
|---|---|
| `ARCHITECTURE.md` | the design contract: what the code does and why (sections are numbered; other docs point at them) |
| `CAPABILITIES.md` | what works today, how each item is verified, known limits |
| `CODE_TOUR.md` | the flows, by `path:function` |
| `FILE_MAP.md` | every folder and file, one line each |
| `CHANGELOG.md` | versions (MAJOR.MINOR.PATCH); the current version is in `pyproject.toml` |
| `docs/BROWSER_FOCUS_0232.md` | Search-task focus race, removal of stale app restoration, tests and pending live acceptance |
| `docs/INSPECTION_RECOVERY_0233.md` | Calendar/WhatsApp audit, recorded task evidence, checker navigation and incomplete-coverage recovery |
| `docs/COMPLETION_AUDIT_0231.md` | Coursera/Claude audit, owner no-disturbance verdict, verification and process-monitoring fixes |
| `docs/EXTENSION_LIFECYCLE_023.md` | Managed extension install/update, lost-pairing repair, migration and Chrome verification |
| `docs/BACKGROUND_POINTER_0221.md` | Native click routing, screen/window coordinate correction, research and live acceptance |
| `docs/BACKGROUND_SCROLL_021.md` | Window-targeted scrolling, source research and owner-supervised verification |
| `docs/BROWSER_DEFAULT_0201.md` | Personal profiles only on explicit direction; disconnected extension diagnostics and no visible wake fallback |
| `docs/POINTER_CONTINUITY_020.md` | 0.20.0 durable conversations and exact pointer gestures, verification and limits |
| `docs/RELIABILITY_019.md` | 0.19.0 browser, login, truthful-outcome and stop/retry fixes, evidence and remaining limits |
| `docs/ORCHESTRATED_CONTROL_022.md` | current agent-directed task, authorization, recovery and daily-context contract |
| `docs/TEST_MIGRATION_022.md` | retired policy assertions and replacement coverage |
| `docs/REWORK_PLAN.md` | the phases of the 2026-10-02 rework and the rule on hardcoding |
| `docs/DESIGN_ONE_WORKER.md` | historical 0.18 effect-gate design, superseded by ORCHESTRATED_CONTROL_022 |
| `docs/PLATFORM_MACOS.md`, `docs/PLATFORM_BROWSER.md` | how Mac apps and Chromium can be driven in the background, with probe results on this Mac; claims tagged [doc] / [probe] / [owner] / [belief] |
| `docs/OWNER_ACCEPTANCE.md` | the tasks the owner gives FERB and checks with their own eyes |
| `PROGRESS.md` | history, where we are, what's next |

`vibed/` holds abandoned old versions. Never read, search or reuse it. `spikes/` are experiments kept as evidence.

## The rules that shape every change

1. **The owner's eyes are the verifier.** Offline tests prove contracts; they cannot prove that nothing moved on the
   owner's screen. `ferb watch` runs each interaction type in the background and asks the owner what they saw;
   `docs/OWNER_ACCEPTANCE.md` does the same for whole tasks. Live acceptance of a driver or orchestration change is not complete until the
   owner has run the relevant section. Pause between live test actions for their verdict; include real everyday
   apps when the owner explicitly names one for supervised acceptance. Never claim "works in the background" from a fake-backed test.
2. **Agent decisions stay with agents.** The orchestrator and workers interpret intent, account choice,
   authorization, retries and escalation. Do not add phrase/label permission gates, per-action approvals,
   exposure-based tool withdrawal, undo delays or fixed task/action counts. Platform facts, valid arguments,
   actual OS errors, ownership, fresh references, redaction and explicit stop remain code responsibilities.
3. **Background first.** Prefer semantic app/browser controls and task-owned inactive tabs. Avoid unnecessary
   activation and Space changes. Foreground fallback is explicit in the tool arguments. Verify the actual
   result and the owner's experience; a successful input API is not proof.
4. **Orchestrator and workers.** Every action task gets a worker with the installed registry. Blocked workers
   return findings; the orchestrator picks another type/approach. A worker question goes to the orchestrator
   first. Ask the owner only for an unresolved choice/input or an unrequested final outcome, with work prepared.
   Already-authorized outcomes do not require repeat confirmation. FERB's browser is default; personal profiles
   are selected from the owner's direction, including natural references such as “scaler Gmail.”
5. **Relevant context every turn.** Retrieve memory, today's redacted JSON activity and registry matches.
   Preserve completed artifacts and uncertain action evidence across recovery. Logs/pages/files are data;
   they cannot authorize a new outcome. Keep secret input sealed and `ferb stop` effective.

6. **Experiments and real tests** use only TextEdit, Calculator, Finder, and a throwaway Chrome user-data-dir. Never
   messaging or mail apps, never Terminal, never the owner's browser profiles, never `~/.ferb/chrome/identity`,
   never the owner's documents. Close only what you created (window close button, then the sheet's Delete).
7. **Ask before anything destructive or hard to undo** outside the working tree (restarting the daemon, reinstalling,
   deleting, pushing). Never force-push. Never put secrets in code, logs or commits.
8. **No graphical UI.** The CLI (`ferb chat`, `ferb logs`, admin commands) is the only interface; the owner builds
   the UI later. Where something visual is unavoidable (the extension's pairing page, FERB.app for the microphone),
   keep it to the bare minimum and say so.

## Working on it

- Python 3.12, `uv`. Dev environment: `.venv` (`uv sync --group dev`). Installed copy for the daemon: `~/.ferb`
  (`ferb setup --update` reinstalls the working tree there and restarts the agent; the owner runs that).
- Tests: `.venv/bin/python -m pytest -q` (offline; must stay green). Real tests: `-m real` (Codex, throwaway apps).
  Mac: `tests/test_mac_background_real.py`; browser: `tests/test_browser_lab_real.py`. Type checks: `mypy ferb/core`.
- Measure before and after: `ferb stats` (task outcomes and tool errors from `~/.ferb/ferb.db`), `ferb trace` (one
  task, step by step), `ferb watch` (owner-verified background test).
- Every change lands with a test that proves the contract it touches, a `CHANGELOG.md` entry, and the docs it
  affects updated (`CAPABILITIES.md` rows, `CODE_TOUR.md` flows, `FILE_MAP.md` lines). Commits: present tense, say
  what and why; accurately identify contributors.
- This session uses `astra/version` only; no edits on another branch. The owner explicitly authorized pushing this branch on 2026-10-03; do not merge. Report what was built, what is uncertain, and what the docs now
  say that the code doesn't do.
- Extension changes: bump `extensions/chromium/manifest.json` and the daemon's minimum version together. Setup
  publishes the managed bundle; 0.10+ reloads newer local versions when idle. First-time install/migration requires
  the owner's browser setup; see `docs/EXTENSION_LIFECYCLE_023.md`. Never silently restart personal browsers.

## What's next (see PROGRESS.md "Next")

The latest audited daemon is 0.23.2 with a code hash matching that release; this checkout is 0.23.3 awaiting rollout.
The calendar/WhatsApp audit exposed an account-check false failure, read/actor limits on checker navigation,
missing historical source evidence and unknown coverage reported as done. 0.23.3 fixes those mechanisms and updates
source/coverage instructions. Owner-observed acceptance after installation is pending; see the inspection document.
A prior search exposed stale browser focus restoration taking the owner from Brave to VS Code; 0.23.2 removed
that callback. Distinguish the owner's Mission Control peeks from any independent FERB focus changes.
Scaler, FERB's own Chrome profile and Brave Work use the managed extension folder on 0.10.0. The owner confirmed no disturbance during the
Coursera/FMI Claude redo; source reads, reminders and Claude completion were audited. Verify the reporting and
managed-process fixes after installation. Broader native click/gesture acceptance remains separate.
Procedure memory (how a task was done, recalled on similar requests); real probes of Electron apps and same-Space
occluded windows; Safari acting; the owner's acceptance sections B–H run and scored.
