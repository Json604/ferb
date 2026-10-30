# Working on FERB (for Codex and any other agent)

`CLAUDE.md` is the working agreement for every agent in this repository. Read it first; this file adds to it. The
source of truth is `notes/` (one file per area, with the owner's decisions, plus `fix-order.md`). Where this file,
`CLAUDE.md` and `notes/` differ, ask the owner.

## The owner's working preferences

- **The owner audits the code and the plans.** Before saying how code behaves, read the code path and cite
  `file:line`. Before saying what happened, read the trace (`ferb trace TASK --full`/`--json`) or the database. Say
  "I inferred" or "I didn't verify" for anything unchecked, and correct an earlier wrong claim as soon as you find it.
- **"Already built" needs evidence**: the mechanism step by step (trigger → code path → storage → where it's used,
  with `file:line`) plus live evidence that it runs (counts, dates, errors from traces).
- **Before saying "impossible"**, when the owner says something used to work, read the old trace end to end.
- **Limits are the owner's decisions**: size caps, counts, budgets, retention, rate limits. Bring the risk and a
  recommended number to the owner; never introduce one on your own.
- **Sequencing and method are yours**; what a feature does, who/which, and product decisions go to the owner.
- **Quality, goals and security decide designs**, never build time or effort. Prefer one shared path over per-case
  patches; a correction about the owner belongs in FERB's memory, not in a prompt.
- **Live tests (`-m real`) use the Codex runtime**; `-m real_claude` runs only when the owner runs it.
- **North star:** an agent that controls the Mac the way the owner can, in the background, and keeps learning by doing.
- **The app is the product**; there is no user-facing CLI.

## Branch

Work happens on the branch the owner names (currently `ui/ux`, which holds the owner's UI work with Codex). Commit and
push only when the owner asks; never force-push. At the end of the rebuild the branch is merged into `main` (a
fast-forward while `main` has nothing the branch lacks).

## The UI (`ui/`, the owner's side, often built with Codex)

- The notch: `ui/mac/FERBNotch/Sources/FERBNotch` (`NotchView.swift` and related); shared logic in `Sources/NotchCore`.
- The setup window: `SetupView.swift`, `SetupModel.swift`. The FERB window and Settings: `FERBWindowStore.swift`,
  `WindowPages.swift`, `SettingsPages.swift`.
- The app talks to the daemon only through its RPCs (`ferb/daemon/main.py`, method table; roles in
  `ferb/daemon/auth.py`). A UI change that needs a new or changed RPC is a daemon change: record it in the area's
  notes file.
- `swift test` in `ui/mac/FERBNotch` after `source scripts/swiftpm-anonymous.sh`; SwiftPM must never use the owner's
  keychain.

## Running and building

- **What the owner runs:** the release `FERB.app` in `/Applications`; the daemon runs from
  `/Applications/FERB.app/Contents/Helpers/FERB.app`, data in `~/.ferb`. `ferb setup --update` from a checkout
  switches the daemon to the development venv, so don't run it while testing the release app. Never install or
  restart the owner's daemon without the owner's authorization.
- **Release build:** `source scripts/swiftpm-anonymous.sh; SIGN_IDENTITY=release scripts/build-app.sh --arch arm64
  build/app-arm64`; stop the daemon (`launchctl bootout gui/$(id -u)/com.ferb.daemon`) and quit FERB.app; move the
  old app to the Trash, `ditto build/app-arm64/FERB.app /Applications/FERB.app`, then open it. Disk images:
  `SIGN_IDENTITY=release scripts/build-dmg.sh`. (The service registration moves into the app per notes/daemon.md
  decision 2; update these steps when it lands.)
- **Notch-only iteration:** `ui/mac/FERBNotch/build.sh` makes a separate development notch; never run it next to the
  release app's notch.
- **Signing:** the "FERB Release Signing" certificate (SHA-1 in `scripts/release-cert.sha1`) and the Sparkle update
  key live in the owner's login keychain. Never commit, print or move the private keys: together they could ship a
  fake update to every user.
- **Python tests:** `nice -n 10 uv run pytest -q` (each test is capped by `tests/support/runaway.py`); never two
  full suites at once.
