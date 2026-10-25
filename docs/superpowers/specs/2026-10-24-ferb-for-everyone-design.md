# FERB for everyone: the Mac app, setup, updates and packaging — design (2026-10-24)

Status: approved by the owner on 2026-10-25 for the launch: "all 7 parts in full, just design them bare min. focus on
correct working" (direction approved 2026-10-24: "ok skip the $99 for now, do the free ones"; answers below). Every
screen is a bare-minimum working design; the owner polishes the look later with Codex. Decision id: L34. One
implementation plan per phase, built after the 2026-10-25 work in flight.

## Why

FERB is about to go to other people. The 2026-10-24 audit found what a stranger installing FERB.app would hit:
setup is `ferb setup` running in a console window with `[y/N]` prompts (`ui/mac/FERBNotch/Sources/FERBNotch/
AppSetupController.swift`; `README.md:26-28`); permissions are granted by hand ("click +, press ⌘⇧G, paste a path",
`ferb/cli/setup.py`); notifications post through `osascript` as Script Editor (`ferb/daemon/main.py:2299-2303, 4280`);
the menu bar always says "Connected" (`PanelCoordinator.swift`); there is no settings window beyond sound, no
schedules/memory/skills UI, no launch at login for the app, release builds are ad-hoc signed so users' permissions
reset on every update (`scripts/signing.sh:11`, `README.md:533`), the app is Apple-silicon only
(`scripts/build-app.sh:35`) and about 666 MB (`README.md:535`).

The owner chose to skip Apple's paid Developer Program for now. Without it, a user clicks **Open Anyway** once in
System Settings → Privacy & Security on first open (macOS 15+ removed the right-click bypass); everything else below is
free.

## Owner decisions (2026-10-24)
- Free items only; no Developer ID or notarization for now.
- Updates install by themselves ("cant this be that, a new update came so update it??").
- Intel support as a separate download: two DMGs ("Two DMGs").
- The notch stays the main, quick interface; detailed things live in a regular window ("i think if some more detailed
  things then a regular window is good for this and notch as primary main use").

## What a new user experiences

1. **Download and install.** The release page offers `FERB-AppleSilicon.dmg` and `FERB-Intel.dmg`. The disk image
   window shows FERB, an arrow to Applications and one line: "First open: System Settings → Privacy & Security → Open
   Anyway". Every release is signed with one stable FERB certificate.
2. **First open.** macOS blocks once; the user clicks Open Anyway and enters their password.
3. **Setup window** (native, Apple onboarding style: one title, two lines, one primary button, progress dots):
   Welcome (what stays on the Mac, what reaches the AI service redacted) → Choose your AI (Sign in with ChatGPT / Use
   Claude / API key; live status) → Permissions (one row each: Accessibility, Screen Recording, Full Disk Access,
   Automation, Notifications, Microphone optional; one-line reason; **Grant** opens that pane; the row ticks itself when
   granted) → Browsers (each browser and profile: not installed / installed / on / paired; **Add to Chrome** opens the
   Web Store page; pairing is automatic when the user is in that profile, L25) → Getting to know you (the persona line:
   Save / Edit / Skip) → Done (into the notch; three tips).
4. **Everyday use.**
   - The notch: chat, tasks, questions (short line, Details), loops ("Keeping: …"), the Noticed count, FERB's cursor.
   - Real FERB notifications (FERB's name and icon) for a waiting question while the notch is closed, reminders,
     alerts, a long task finishing, a loop pausing or backing off; clicking opens the notch at that item; Focus modes
     respected; categories switchable in Settings.
   - The menu bar icon shows real state: working, needs you, paused, or "FERB isn't running — Restart". Menu: Open FERB,
     Open FERB Window, Pause FERB, Settings…, Quit.
   - **The FERB window** (regular window, sidebar): Tasks (history with results and Inspect), Schedules & Reminders
     (each with its rule, next run, last result; pause, resume, cancel), Noticed (the notices list), Memory (what FERB
     knows; confirm or forget; the existing dashboard's data natively, or the dashboard page at first), Settings.
   - **Settings** (⌘,; also in the window): General (launch at login, notch shortcut, FERB's cursor, sounds, automatic
     updates), AI (service, sign in/out), Permissions (live status, fix buttons), Browsers, Notifications, Privacy (apps
     and sites FERB ignores, what's recorded, retention, Pause recording), Advanced (logs, export a diagnostic file,
     install the command-line tool, uninstall).
5. **Launch at login.** FERB.app registers itself as a login item; the background service already starts at login.
6. **Updates.** Sparkle (free, open source) checks the GitHub release feed about once a day, downloads a new version,
   verifies it with FERB's own update key (EdDSA, independent of Apple) and that it carries the same FERB certificate,
   and installs it when FERB is idle (no running task), restarting the background service; then the notch says "Updated
   to <version>". A Settings switch turns automatic updates off (then: "Update available — Install"). Only the public
   release list is requested; nothing about the user is sent.
7. **Power users.** The bundled `ferb` command stays; Advanced has "Install command-line tool"; a checkout install keeps
   working.

## Design (by phase; each phase is its own plan, reviewed tasks, owner live check on a second macOS user account)

### Phase A — Measure and shrink, two architectures
- Measure the bundle by component (Python and site-packages, node, Codex, Swift binaries, voice helper, resources) and
  record it in the plan; remove what FERB doesn't use at runtime (tests, docs, caches, unused optional dependencies;
  node if Codex's native binary can be launched directly). The size target is set from the measurement, with the owner.
- `build-app.sh` builds arm64 and x86_64 separately (Python for each architecture, Codex's darwin-x64 package, Swift
  targets per architecture); `build-dmg.sh` produces both DMGs. Intel is tested on an Intel Mac or Rosetta-free VM.

### Phase B — Stable signing
- Release builds sign with one long-lived self-made code-signing certificate (not ad-hoc); `signing.sh` documents
  creating and backing it up. The designated requirement pins that certificate, so TCC grants survive updates.
- Verified first with a two-build test on a second macOS user: grant permissions to build N, install build N+1 signed
  with the same certificate, confirm the grants hold. If TCC doesn't honour it there, stop and report to the owner.

### Phase C — Native setup
- A SwiftUI setup window replaces the console (`AppSetupController.swift`). Each step reads and acts through daemon
  RPCs (permission status, runtime sign-in status, browsers and pairing, persona), added where the CLI holds the logic
  today (`ferb/cli/setup.py`, `ferb/device/permissions.py`, `ferb/device/extensions.py`); the CLI keeps working on the
  same functions.
- Permissions: status polled per service; Grant opens the exact System Settings anchor; Full Disk Access shows the app
  to add when macOS needs it.

### Phase D — Notifications and menu bar state
- FERB.app posts notifications with `UNUserNotificationCenter` (categories: question, reminder, alert, task finished,
  loop); the daemon stops using `osascript` when the app is installed and running, and keeps it as the fallback when
  no app is connected. Clicking a notification opens the notch at its item.
- The menu bar icon and menu reflect the daemon's real state (connected, working, needs you, paused, not running).

### Phase E — The FERB window and Settings
- A regular window with a sidebar: Tasks, Schedules & Reminders, Noticed, Memory, Settings. It uses the RPCs that
  already exist and the app never called (`schedules.*`, `reminders.*`, `memory.*`, `verdict.set`, `stop.engage`,
  `doctor.check`, `runtimes.check`, `sense.pause/resume`; audit 2026-10-24) plus the Noticed list.
- Settings as listed above; "Export a diagnostic file" writes a local, secret-redacted bundle (sending it anywhere is a
  later owner decision).

### Phase F — Launch at login
- `SMAppService.mainApp.register()` from Settings → General, on by default after setup. Verified with the stable
  self-made signature; if macOS refuses it without a Developer ID, fall back to a user LaunchAgent for the app and say
  so in the plan.

### Phase G — Automatic updates
- Sparkle 2 embedded in FERB.app; appcast hosted with the GitHub releases; EdDSA key generated once and kept by the
  owner (never in the repo); updates are applied only when no task is running, then the service restarts; a running
  loop resumes after the restart (L31 restart behaviour). Settings switch for automatic updates (default on).

## Not in this design
- Apple Developer ID, notarization, Homebrew cask (needs notarization since Homebrew 5.0).
- Lid-closed insomnia (L32): designed separately; its root helper likely needs a Developer ID.
- Sending diagnostic reports to the developer: a later owner decision (what may leave the Mac, the address).

## Testing
- Each phase: offline tests (Python RPCs; Swift views and controllers), `swift test`, the full suite, and the owner's
  live check on a fresh second macOS user account (install from the DMG, setup, permissions, a task, an update).
- L34 registered in `docs/0.27_DECISIONS.toml`; README (install and setup), ARCHITECTURE.md and CHANGELOG.md updated per
  phase.
