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
| `docs/POINTER_CONTINUITY_020.md` | 0.20.0 durable conversations and exact pointer gestures, verification and limits |
| `docs/RELIABILITY_019.md` | 0.19.0 browser, login, truthful-outcome and stop/retry fixes, evidence and remaining limits |
| `docs/REWORK_PLAN.md` | the phases of the 2026-10-02 rework and the rule on hardcoding |
| `docs/DESIGN_ONE_WORKER.md` | the task loop and the gate at the action (effect classes) |
| `docs/PLATFORM_MACOS.md`, `docs/PLATFORM_BROWSER.md` | how Mac apps and Chromium can be driven in the background, with probe results on this Mac; claims tagged [doc] / [probe] / [owner] / [belief] |
| `docs/OWNER_ACCEPTANCE.md` | the tasks the owner gives FERB and checks with their own eyes |
| `PROGRESS.md` | history, where we are, what's next |

`vibed/` holds abandoned old versions. Never read, search or reuse it. `spikes/` are experiments kept as evidence.

## The rules that shape every change

1. **The owner's eyes are the verifier.** Offline tests prove contracts; they cannot prove that nothing moved on the
   owner's screen. `ferb watch` runs each interaction type in the background and asks the owner what they saw;
   `docs/OWNER_ACCEPTANCE.md` does the same for whole tasks. A change to a driver or to policy is not done until the
   owner has run the relevant section. Never claim "works in the background" from a fake-backed test.
2. **Hardcoding** is allowed for platform facts (AX role names, keycodes, CDP methods, browser bundle ids) and safety
   floors (destructive shell patterns; pay / delete / account-security always ask; the one floor word list in
   `ferb/policy/effects.py`). It is not allowed for deciding what the owner meant or what a control does: no verb
   tables, no label word lists that grant or refuse, no phrase lists over the owner's words, no per-app branches.
   App-specific behaviour is learned (`~/.ferb/device/apps/<bundle>.json`, `python -m ferb.act.mac.probe`).
3. **Background first.** Mac: Accessibility actions and attributes, menu items, keystrokes to the app's process;
   pixel clicks are the last resort and the foreground path needs `allow_foreground` on the call. Browser: FERB's own
   minimized window, task tabs never active, a debugger session per task tab, trusted input. Never `activate`, never
   quit the owner's apps, never ⌘Tab.
4. **One worker per request, gated at the action.** The model decides what an action means (`effect` + `why` on
   every acting call); code floors can only raise it; sends ask once per target unless the owner named it; pay and
   destroy ask every time; `owner_ask` lets the worker ask the owner mid-task. Don't reintroduce pre-classification.
5. **Safety floors never move**: identity split (owner vs FERB), scopes by identity, exposure after untrusted reads,
   the hard floor, the undo hold, the duplicate-send check, sealed secret fills, the shell jail, redaction, `ferb stop`.
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
  what and why; end with `Co-Authored-By: Claude <model> <noreply@anthropic.com>` when an agent wrote it.
- Work on a branch; the owner merges and pushes. Report what was built, what is uncertain, and what the docs now
  say that the code doesn't do.
- Extension changes: bump `extensions/chromium/manifest.json` and the daemon's minimum version together; the owner
  reloads the unpacked extension in each paired profile.

## What's next (see PROGRESS.md "Next")

Procedure memory (how a task was done, recalled on similar requests); real probes of Electron apps and same-Space
occluded windows; Safari acting; the owner's acceptance sections B–H run and scored.
