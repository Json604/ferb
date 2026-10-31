# Working on FERB (for Codex and any other agent)

Read `CLAUDE.md` first: it is the working agreement for every agent in this repository, Codex included, and its rules
apply here word for word. This file doesn't restate them; it adds what a non-Claude agent wouldn't otherwise know.

The source of truth is `notes/`: one file per area with the owner's decisions, what exists, the target design and the
fix list; `notes/fix-order.md` is the order of work and `notes/test-tasks.md` the real tasks and the step-13 live
checklist. Where this file, `CLAUDE.md` and `notes/` differ, name both and ask the owner. Update the area's notes file
when a fix lands or a decision changes, so the next agent (Claude or Codex) starts from current notes.

## The owner's working preferences
- **The owner audits the code and the plans.** Before saying how code behaves, read the code path and cite
  `file:line`. Before saying what happened, read the trace (`ferb trace TASK --full`/`--json`) or the database. Say
  "I inferred" or "I didn't verify" for anything unchecked, and correct an earlier wrong claim as soon as you find it.
- **"Already built" needs evidence**: the mechanism step by step (trigger → code path → storage → where it's used,
  with `file:line`) plus live evidence that it runs.
- **Before saying "impossible"** about something the owner says used to work, read the old trace end to end.
- **Sequencing and method are yours**; what a feature does, who/which, limits and product decisions are the owner's.
  Your own rulings are never cited as the owner's decision.
- **A behavior rule the owner states goes in the shared path every route uses**, plus a sweep of prompts, facts and
  errors; never tool by tool.
- **North star:** an agent that controls the Mac the way the owner can, in the background, and keeps learning by doing.

## Branch
Work happens on the branch the owner names (currently `ui/ux`). Commit and push only when the owner asks; never
force-push. The checkout can be shared by parallel agents: never `git stash`, never reset others' work, and check
`git diff --cached` before committing.

## The UI
`ui/` is the owner's side (often built with Codex); `ui/AGENTS.md` covers it. The app talks to the daemon only through
its RPCs (`ferb/daemon/main.py`, roles in `ferb/daemon/auth.py`); a UI change that needs a new or changed RPC is a
daemon change, recorded in the area's notes file. `swift test` in `ui/mac/FERBNotch` after
`source scripts/swiftpm-anonymous.sh`; SwiftPM must never use the owner's keychain.

## Running and building
- **What the owner runs:** the release FERB.app in /Applications; its background service is registered by the app
  (SMAppService, notes/daemon.md decision 2), data in `~/.ferb`. Never install, restart or stop the owner's daemon,
  browsers or FERB.app without the owner's authorization, and never write to `~/.ferb` from a test.
- **Release build:** `source scripts/swiftpm-anonymous.sh; SIGN_IDENTITY=release scripts/build-app.sh --arch arm64
  build/app-arm64` (it needs Command Line Tools / Xcode that can read the current macOS SDK). To install, with the
  owner's go: quit FERB.app, move the old app to the Trash, `ditto build/app-arm64/FERB.app /Applications/FERB.app`,
  open it; its launch updates the service. Disk images: `SIGN_IDENTITY=release scripts/build-dmg.sh`.
- **A checkout's daemon:** `uv run ferb setup --update` installs a development LaunchAgent; it refuses while FERB.app's
  service is registered.
- **Signing:** the "FERB Release Signing" certificate (SHA-1 in `scripts/release-cert.sha1`) and the Sparkle update
  key live in the owner's login keychain. Never commit, print or move the private keys: together they could ship a
  fake update to every user.
- **Python tests:** `nice -n 10 uv run pytest -q` (each test is capped by `tests/support/runaway.py`); never two full
  suites at once. Live tests (`-m real`) use the Codex runtime and need the owner watching.
