# Rework plan: human-like control, background first, learning by doing

> Historical design. The owner replaced action/identity/exposure/phrase gates in 0.22; see [the current contract](ORCHESTRATED_CONTROL_022.md). Platform evidence below remains version-specific.


Branch: `rework/human-control`. Started 2026-10-02 from the independent audit (findings in the audit artifact;
run-history numbers from `~/.ferb/ferb.db`: 11 of 66 task outcomes done, reader-policy blocks the top tool error).

## The rule on hardcoding

Hardcoding is allowed for **facts about the platform** and **safety floors**. It is not allowed for **deciding
what the owner meant** or **what a control does**.

| Allowed (facts / floors) | Not allowed (judgement) |
|---|---|
| AX role and attribute names, virtual keycodes, bundle ids of browsers, CDP method names | Verb → action-kind tables (`IMPLIES`), "risky label" word lists, "search box" label regexes |
| Destructive shell patterns, the hard floor (pay / delete-permanent / account-security), undo hold | "Owner named their browser" phrase lists, service catalogues deciding app vs web |
| Identity split (owner vs FERB), scopes by identity, exposure after untrusted reads | Per-app behaviours written into generic branches (WhatsApp's search box, Gmail's Send) |

Where the code needs to know how a specific app behaves, it **learns it by probing** and stores a per-app
capability profile; it never ships a hand-written one.

## Phases

### 0. Branch and plan (done)

Status 2026-10-02: phases 1–4 done (see PROGRESS.md "Where we are"); phase 5 not started; owner acceptance B–H pending.
Follow-up on `astra/version`: 0.19.0 recovery contracts are in `RELIABILITY_019.md`; live owner acceptance remains pending.

### 1. Learn the platform before touching control
Two documents, written from Apple / Chromium documentation plus live probes on this Mac (safe apps only:
TextEdit, Calculator, Finder, the FERB Test Chrome profile; never messaging apps, never the identity session):
- `docs/PLATFORM_MACOS.md`: how Mac apps are built (AppKit, SwiftUI, Catalyst, Electron, Chromium, Qt/Java) and what
  each exposes: the Accessibility tree and its actions, Apple Events, menus, windows and Spaces, the event system
  (HID tap vs `CGEventPostToPid` vs `AXUIElementPostKeyboardEvent`), responder chain and key windows, secure
  input, TCC permissions, what works with the app in the background per mechanism.
- `docs/PLATFORM_BROWSER.md`: extension model (MV3, isolated worlds, service-worker lifetime), CDP via
  `chrome.debugger` (trusted input, focus emulation, dialogs, screenshots, infobar), background-tab throttling,
  trusted vs untrusted events, windows and tab groups, profiles, native messaging.
- `ferb/act/mac/probe.py`: a runnable probe that, for one running app, tries every background mechanism on a
  harmless target (a menu item that opens nothing, a text field it created, a scroll) and records which worked.
  This becomes the agent's own tool for learning a new app (phase 5).

### 2. Stop the bleeding (verified bugs, each with a test)
- `do script` added to the AppleScript forbidden list (Terminal shell escape).
- Codex run token moved off the command line (token file, 0600, path in env).
- `_background_ok` poison flag: reset on any later background success; never set by a key with no expected effect.
- `ledger_done` ignores `expected`; a code check that can't decide falls to the model verifier, never fails the task.
- Extension: reconnect backoff really backs off; `wait` is cancellable and returns `met: false` on timeout.
- `worker.md` matches the gateway on `needs_owner_approval` (retry after approval).
- Shell: output paged by offset from the start; Homebrew and user bins on PATH under launchd.
- A per-request outcome line in `ferb logs` (done / failed / needs_user, model runs, tool errors by reason).

### 3. Drivers: background first, foreground by explicit request
Mac: pid-targeted mouse events; app-element hit test; `AXSelectedTextRange` + `AXSelectedText` editing; snapshot
by available actions, not role allowlist; address windows, sheets, popovers and open menus; scroll, drag,
popup select, slider increment; full keycode table; `AXEnhancedUserInterface` + `AXManualAccessibility` per pid;
one cached tree walk per snapshot. Foreground only with `allow_foreground=true` on the call.
Browser: FERB's own minimized window per profile; debugger attached per task-tab for its life; pointer sequences
and modifier keys; viewport-aware snapshots with in-view flag and scroll position; container scroll; JS dialogs.
Real tests: act on a backgrounded TextEdit and on a background Chrome tab and assert the frontmost app never changed.

### 4. One worker, gate at the action
Delete the reader/actor split, typed reader fields, `IMPLIES`, label regexes, the owner-browser phrase gate, the
own-words rule for names read off the screen, the spelling corrector. One worker per request with every tool its
identity allows. Every act call declares `effect` (navigate / edit / send / pay / destroy) and `why`; reversible
effects run; send / pay / destroy ask once per (request, effect, target) through the existing approvals, auto-
approved when the owner's message names the target. Code floor: a control whose label literally says Send / Pay /
Delete can't be declared navigate. `ask_owner` tool blocks the worker until the owner answers in chat.
Verification: code checks by evidence (files, URLs, ledger has a done consequential action), model verifier for
the rest. Acceptance set: the WhatsApp search, Gmail open-email and Spotify like requests from the run history.

### 5. Learning by doing
Procedure memory: after a done task, store "how I did X in app Y" (tool sequence, controls, what failed first),
keyed by app / site and goal shape; recalled into the worker prompt on similar requests. Per-app capability
profiles from the phase-1 probe, updated by every real action's outcome.

## Assurance
Every change lands with a test. Platform claims are proved by probes on this Mac before the driver relies on
them; what can't be probed here (e.g. an app class not installed) is marked unverified in the docs, not assumed.
