# Packaging Phase C: native setup (L34) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A native SwiftUI setup window in FERB.app (Welcome → Choose your AI → Permissions → Browsers → Getting to
know you → Done) replaces the `ferb setup` console, reading and acting only through daemon RPCs that share their code
with the CLI.

**Architecture:** The first step still installs the background service with FERB.app's own Python, now
non-interactively: Welcome runs the bundled `ferb setup --service`, which runs the same `Installer.install()` as today
(signature and location checks, `~/.ferb`, `~/Library/LaunchAgents/com.ferb.daemon.plist`, `launchctl bootstrap`) and
exits 0 once the daemon answers on `~/.ferb/run/ferb.sock`. Every later step reads and acts through new CLI-role RPCs
(`setup.permissions`, `setup.permission_request`, `setup.finish`, `runtimes.status`, `runtimes.use`,
`runtimes.sign_in`, `setup.browsers`, `setup.browser_open`) plus existing ones (`automation.request`, `setup.persona`, `chat.approve`,
`chat.answer`, `runtimes.check`). The RPC bodies call functions added next to the CLI's logic
(`ferb/device/permissions.py`, `ferb/device/extensions.py` with `ferb/act/browser/autoinstall.py`,
`ferb/cli/runtimes.py`), and `ferb setup` keeps working on those same functions. FERB.app's own permissions
(Notifications; Microphone with Speech Recognition for dictation) are read and requested by FERB.app itself. A
`SetupModel` (testable with fakes) drives a plain SwiftUI `SetupView` in a regular window.

**Tech Stack:** Python 3.12 (asyncio daemon, PyObjC AX/Quartz, ctypes IOKit), Swift 6 toolchain in Swift 5 language
mode, SwiftUI/AppKit, UserNotifications, AVFoundation, Speech; `uv run pytest`, `swift test` in `ui/mac/FERBNotch`.

**Spec:** `docs/superpowers/specs/2026-10-24-ferb-for-everyone-design.md` (L34, approved 2026-10-25: "all 7 parts in
full, just design them bare min. focus on correct working"). Read "What a new user experiences" step 3 and "Phase C".

## Global Constraints

- Read `CLAUDE.md` first. Never read, search or reuse `vibed/`. Work on `main`; every commit message cites `L34`
  (plus `CQ5` where Full Disk Access is gated). Commit only the task's own files by path (`git add <paths>`, then
  `git diff --cached --stat`): other agents keep uncommitted work in this checkout.
- Every screen is bare minimum (spec): plain SwiftUI (`VStack`, `HStack`, `Text`, `Button`, `SecureField`,
  `TextField`, `Picker`, `ProgressView`, `ScrollView`); no custom colors, materials, images or animation. The owner
  styles it later with Codex. The notch stays the main interface (rule 18).
- Background first (rule 17): the window appears only at first launch, from **Set Up FERB…**, or when a quiet update
  finds a required grant missing or fails. System Settings, Finder and a browser profile open only on the owner's
  click. A quiet update (new build, stopped service) shows nothing when it succeeds and every required grant holds.
- Secrets (rule 14): an API key typed in the window goes from the `SecureField` straight to `/usr/bin/security -i` on
  its stdin; never an argument, never to the daemon, a log, `NSLog`, `UserDefaults`, a `@Published` property or the
  conversation archive. Only its presence is checked afterwards. `runtimes.check`'s `detail` is redacted. FERB's
  Claude sign-in (Task 10) never handles the credential at all: the pinned Claude Code writes it to the login Keychain
  itself; the one-time code the owner may paste from the sign-in page reaches only Claude Code's stdin, is added to
  the daemon's redactor, and is never journaled, traced or stored.
- Whose grant each row is, and where Grant goes (the exact anchors):

  | Row | Whose grant (TCC identity) | Read by | Grant button |
  |---|---|---|---|
  | Accessibility | FERB's agent app `FERB.app/Contents/Helpers/FERB.app` (`com.ferb.daemon`, runs the Python daemon) | daemon: `AXIsProcessTrustedWithOptions(prompt: False)` | `setup.permission_request` → `AXIsProcessTrustedWithOptions(prompt: True)`; opens `x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility` |
  | Screen Recording | agent app | daemon: `CGPreflightScreenCaptureAccess` | `CGRequestScreenCaptureAccess`; `…?Privacy_ScreenCapture`; then "Restart FERB to check" (macOS applies it after a restart) |
  | Full Disk Access | agent app | daemon: TCC.db read probe | reveals the agent app in Finder (drag it into the list) and opens `…?Privacy_AllFiles`; blocked until containment works (CQ5, rule 12) |
  | Input Monitoring (optional) | agent app | daemon: `IOHIDCheckAccess` | `IOHIDRequestAccess`; `…?Privacy_ListenEvent` |
  | Paste from Other Apps (optional; clipboard, I3) | agent app | daemon: the pasteboard's `accessBehavior` (never the contents) | `setup.permission_request {permission: "clipboard"}` reads it once so macOS asks (only while it hasn't asked yet); then `…?Privacy_Pasteboard`, where the owner sets FERB to Allow (anchor present in this Mac's System Settings) |
  | Automation | agent app, per target app | daemon: `AEDeterminePermissionToAutomateTarget(ask: false)` | existing `automation.request` (macOS prompt); `…?Privacy_Automation` when off |
  | Notifications (optional) | FERB.app (`com.ferb.FERB`) | FERB.app: `UNUserNotificationCenter` settings | `requestAuthorization`; when off `x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=com.ferb.FERB` |
  | Microphone (optional) | FERB.app (dictation: Microphone + Speech Recognition) | FERB.app: `AVCaptureDevice`, `SFSpeechRecognizer` | `requestAccess` / `requestAuthorization`; when off `…?Privacy_Microphone` or `…?Privacy_SpeechRecognition` |
  | Hearing app sound (optional) | FERB Voice (`Helpers/FERB Voice.app`, `com.ferb.app`) | daemon's hearing probe (starts FERB Voice in the background, no prompt) | `setup.permission_request {permission: "hearing"}` → `open -n -W "FERB Voice.app" --args request-speech`; when off `…?Privacy_SpeechRecognition` |

- Required rows are exactly `accessibility`, `screen_recording`, `full_disk_access` (today's
  `REQUIRED_PERMISSIONS`). The owner can always continue past a missing grant (friction-free); the row stays unticked.
- No new behaviour limits. The only timing values are responsiveness details: the window polls the visible step every
  2 s, each Automation status check waits at most 2 s, and launch asks `setup.permissions` up to 3 times 2 s apart.
- `ferb setup` (interactive) keeps its flow; it now asks macOS to list FERB (`setup.permission_request`) before it
  opens a pane, and shares the row, containment and Automation-candidate functions.
- Don't install anything, touch `~/.ferb`, or restart the owner's daemon or browsers (rule 23). Acceptance is the
  owner's live check on a second macOS user account (rule 20); offline tests never prove it.
- Each task: failing test first, then the focused tests. Before the plan's final commit: `nice -n 10 uv run pytest -q`,
  `uv run mypy ferb/core`, `git diff --check`, and `cd ui/mac/FERBNotch && swift test`.

## Owner decisions (2026-10-25)

The planner's six questions, answered. Only item 4 is the owner's own choice; the rest are the controller's rulings.

1. Keep the optional Input Monitoring and Speech Recognition (hearing app sound) rows (controller). Built in Tasks 1, 2, 7.
2. Add Clipboard (Paste from Other Apps) as an optional row (controller). Built in Task 1 (`clipboard_row`), Task 2
   (the row and its request in `setup.permissions` / `setup.permission_request`), Task 6 (its words).
3. "Task finished" is posted when an answer arrives while the notch is closed, with no time threshold (controller;
   Phase D builds it).
4. Owner: "Build a Claude sign-in like Codex's." Built in Task 10.
5. The menu bar menu is exactly as the spec says (controller; Phase D builds it).
6. Existing installs (set up before this window existed): FERB.app asks once at launch for Notifications while macOS
   hasn't asked yet (controller). Built in Task 7 (`askForNotificationsOnce`) and Task 8 (launch).

Planner's ruling, not the owner's (Task 10): the Claude credential is never routed through FERB's sealed secret path,
because FERB never holds it. The pinned Claude Code writes it into the login Keychain itself (`security -i` on stdin,
item `Claude Code-credentials-<first 8 hex of sha256(~/.ferb/claude-home)>`) and reads it back for every run started
with the same `CLAUDE_CONFIG_DIR`, exactly as `codex login` writes FERB's `~/.ferb/codex-home/auth.json`. Copying it into
a FERB-owned item would add a second copy without adding protection. If the owner wants FERB to own the item, the
alternative is `claude setup-token` (a 1-year token passed to runs on `CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR`), but in
Claude Code 2.1.283 that command is an interactive terminal screen, so FERB couldn't run it without a terminal.

## Review Focus

- **A brand-new user opens FERB.app from the disk image or a translocated copy.** Expected: Welcome's service run fails
  with `check_bundle`'s own fix ("drag FERB.app to Applications…"), the window stays on Welcome showing it, and **Try
  Again** reruns it. Pinned in Task 5 (`test_the_setup_window_refuses_a_disk_image_copy_with_its_fix`) and Task 7
  (`testAFailedServiceRunShowsItsLastLinesAndStaysOnWelcome`).
- **The daemon stops answering mid-setup** (the Screen Recording restart, a crash, launchd's 5 s throttle). Expected:
  the step and its last rows stay, a line says FERB isn't answering, and polling recovers by itself. Pinned in Task 7
  (`testPollingThroughARestartKeepsTheStepAndSaysFERBIsAway`).
- **An API key must not reach the daemon, a log or app state.** Expected: only the Keychain writer sees it; no RPC
  params and no model property hold it. Pinned in Task 7
  (`testAPIKeyGoesOnlyToTheKeychainThenThatServiceIsChosenAndChecked`) and Task 6
  (`testAPIKeyLineCarriesTheKeyAsOneWordOrNothing`).
- **Full Disk Access before containment works** (CQ5). Expected: the row says why and Grant does nothing. Pinned in
  Task 1 (`test_full_disk_access_waits_for_containment`), Task 2
  (`test_full_disk_access_row_waits_for_the_containment_the_daemon_started_with`) and Task 7
  (`testFullDiskAccessBeforeContainmentDoesNothing`).
- **An owner who already set FERB up from the CLI, or reruns Set Up FERB….** Expected: launch shows nothing when
  `permissions_version` ≥ 27; a rerun never reinstalls the service and an already-reviewed profile just continues.
  Pinned in Task 2 (`test_setup_finish_and_cli_setup_both_mark_setup_done`), Task 6
  (`testLaunchOpensTheWindowOnlyWhenSomethingIsLeft`) and Task 7 (`testAnInstalledServiceIsNeverReinstalledFromWelcome`,
  `testLaunchKnowsSetupIsDoneFromTheDaemon`).

## Interfaces for other phases

Phase E (Settings → AI, Permissions, Browsers) consumes these unchanged. All are CLI-role methods (owner's uid), added to
`METHODS[Role.CLI]` in `ferb/daemon/auth.py`, documented in `docs/RPC.md` § "FERB.app's setup window and Settings (L34)".

| Method | Params | Reply |
|---|---|---|
| `setup.permissions` | `{hearing?: bool}` | `{rows: [{id, status, process, required, pane, requestable, restart_to_check, blocked}], automation: [{name, bundle_id, status}], automation_pane: str, grant_target: str, app_panes: {notifications, microphone, speech_recognition}, setup_done: bool}`. `rows` ids: `accessibility`, `screen_recording`, `full_disk_access`, `input_monitoring`, `clipboard` (process `"daemon"`), and `hearing` (process `"voice"`) only with `hearing: true`. `status` ∈ `granted`, `denied`, `not_determined`, `target_not_running`, `unknown`. `blocked` is `""` or why it can't be granted yet. Never prompts. |
| `setup.permission_request` | `{permission: "accessibility" \| "screen_recording" \| "input_monitoring" \| "clipboard" \| "hearing"}` | `{permission, status}`; shows macOS's prompt (`clipboard`: one read so macOS shows its paste alert, journaled `clipboard.access`); journaled `setup.permission_requested`; any other permission is a `-32004` refusal. Call with a long wait (`wait: 120`): `hearing` waits for the owner's answer. |
| `setup.finish` | `{}` | `{setup_done: true}`; sets `[setup] permissions_version = max(27, current)`; journaled `setup.finished`. |
| `runtimes.status` | `{}` | `{roles: {orchestrator, worker, checker}, runtimes: [{name, usable, fix, model, signed_in}]}`; names: `codex`, `claude`, `anthropic`, `openai`, `google`; `signed_in` (Task 10): FERB's own Codex or Claude sign-in finished, or the provider's key is in the Keychain; the `claude` row also has `sign_in: {state: "idle" \| "waiting" \| "signed_in" \| "failed", url, why}`. |
| `runtimes.sign_in` (Task 10) | `{runtime: "claude", code?: str, cancel?: true}` | `{state, url, why}`: starts FERB's own Claude sign-in (or reports the one waiting); `code` passes the code the sign-in page shows to the waiting sign-in; `cancel` stops it. Journaled `runtimes.sign_in` and `runtimes.sign_in_finished {runtime, signed_in}`, never the code or anything Claude Code printed. Any other runtime is refused. |
| `runtimes.use` | `{runtime, roles?: [str]}` | `{roles: {orchestrator, worker, checker}, usable, fix}`; default all three roles; journaled `runtimes.changed`. |
| `setup.browsers` | `{}` | `{store_url: str \| null, extension_dir: str, browsers: [{browser, label, requested, manual, unconfirmed, profiles: [{folder, name, state, ferb_identity, connected}]}]}`; `state` ∈ `not_installed`, `removed`, `installed`, `on`, `paired`. Read only. |
| `setup.browser_open` | `{browser, folder}` | `{opened: url}`; opens FERB's Web Store page (or `chrome://extensions`) in that discovered profile, in front; journaled `setup.browser_opened`. |

Swift pieces Phase E can reuse: `SetupModel` (FERBNotch; `refreshAI()`, `refreshPermissions(probeHearing:)`,
`refreshBrowsers()`, `grant(_:)`, `allowAutomation(_:)`, `restartService()`, `addExtension(_:)`, `signInWithChatGPT()`,
`signInWithClaude()`, `sendClaudeCode(_:)`, `cancelClaudeSignIn()`, `useClaude()`, `saveAPIKey(_:key:)`,
`askForNotificationsOnce(defaults:)`, `startPolling()`/`stopPolling()`), the views `AIStepView`, `PermissionsStepView`,
`BrowsersStepView` (each takes `@ObservedObject var model: SetupModel`), and in NotchCore `PermissionSnapshot`,
`BrowserSnapshot`, `AISnapshot`, `PermissionCopy`, `LaunchctlPlan`. `DaemonService.restart()` (FERBNotch) restarts the
background service (kickstart, else bootstrap); a restart stops running tasks, so a Settings page should say so.

Phase G (updates) may run `ferb setup --service --update` (FERB.app's `Contents/Resources/bin/ferb`) after installing
an update: non-interactive, exit 0 once the daemon answers, `ferb: <reason>` on stderr and exit 1 on failure.

---

### Task 1: Permission rows, requests and shared setup facts in `ferb/device/permissions.py`

**Files:**
- Modify: `ferb/device/permissions.py`
- Test: `tests/test_permissions.py`

**Interfaces:**
- Produces (all in `ferb.device.permissions`):
  - `REQUIRED: tuple[str, ...] = ("accessibility", "screen_recording", "full_disk_access")`
  - `DAEMON_PERMISSIONS: tuple[str, ...] = (*REQUIRED, "input_monitoring")`
  - `REQUESTABLE: frozenset[str] = {"accessibility", "screen_recording", "input_monitoring", "clipboard", "hearing"}`
  - `clipboard_row(state: str | None) -> dict[str, Any]` (owner decision 2: Paste from Other Apps, optional)
  - `RESTART_TO_CHECK: frozenset[str] = {"screen_recording", "full_disk_access"}`
  - `FDA_WAITS: str` (the blocked text)
  - `pane_url(permission: str) -> str`, `notifications_pane_url(bundle_id: str) -> str`,
    `app_panes(bundle_id: str) -> dict[str, str]`
  - `fda_ready(containment: dict[str, Any]) -> bool`
  - `hearing_row_status(hearing: dict[str, Any]) -> tuple[str, str]` (status, blocked)
  - `rows(grants: dict[str, str], containment: dict[str, Any], hearing: dict[str, Any] | None = None) -> list[dict[str, Any]]`
  - `request(permission: str) -> Status` (accessibility / screen_recording / input_monitoring; else `ValueError`)
  - `speech_request_argv(app: Path) -> list[str]`
  - `automation_candidates(apps: Iterable[Any], running: set[str]) -> list[tuple[str, str]]` (name, bundle id)
  - `grant_target(bundle: Path | None, executable: str) -> str`
  - private seams tests replace: `_ax_trusted(prompt: bool) -> bool`, `_request_screen_capture() -> None`,
    `_request_listen_event() -> None`

- [ ] **Step 1: Write the failing tests** (append to `tests/test_permissions.py`)

```python
# ---------- L34 C: the setup window's rows and requests ----------

import os
from pathlib import Path

from ferb.device.profile import AppInfo

PANE = "x-apple.systempreferences:com.apple.preference.security?"


def test_rows_name_whose_grant_and_the_exact_pane() -> None:
    rows = {r["id"]: r for r in P.rows({"accessibility": "granted", "screen_recording": "denied",
                                        "full_disk_access": "denied", "input_monitoring": "not_determined"},
                                       {"profile_ok": True, "shell": True, "proxy": True})}
    assert list(rows) == ["accessibility", "screen_recording", "full_disk_access", "input_monitoring"]
    assert {r["process"] for r in rows.values()} == {"daemon"}
    assert rows["accessibility"]["pane"] == PANE + "Privacy_Accessibility"
    assert rows["screen_recording"]["pane"] == PANE + "Privacy_ScreenCapture"
    assert rows["full_disk_access"]["pane"] == PANE + "Privacy_AllFiles"
    assert rows["input_monitoring"]["pane"] == PANE + "Privacy_ListenEvent"
    assert [i for i, r in rows.items() if r["required"]] == ["accessibility", "screen_recording", "full_disk_access"]
    assert [i for i, r in rows.items() if r["requestable"]] == ["accessibility", "screen_recording", "input_monitoring"]
    assert [i for i, r in rows.items() if r["restart_to_check"]] == ["screen_recording", "full_disk_access"]
    assert rows["accessibility"]["status"] == "granted" and rows["input_monitoring"]["status"] == "not_determined"
    assert all(r["blocked"] == "" for r in rows.values())
    assert P.rows({}, {"profile_ok": True, "shell": True, "proxy": True})[0]["status"] == "unknown"   # not reported


def test_full_disk_access_waits_for_containment() -> None:
    for containment in ({}, {"profile_ok": True, "shell": True},
                        {"profile_ok": True, "shell": True, "proxy": True, "proxy_alive": False}):
        fda = next(r for r in P.rows({}, containment) if r["id"] == "full_disk_access")
        assert fda["blocked"] == P.FDA_WAITS, containment
    assert P.fda_ready({"profile_ok": True, "shell": True, "proxy": True})
    assert P.fda_ready({"profile_ok": True, "shell": True, "proxy": True, "proxy_alive": True})


def test_hearing_row_is_ferb_voices_and_only_when_probed() -> None:
    def hearing(**probe: object) -> dict[str, object]:
        return next(r for r in P.rows({}, {}, dict(probe)) if r["id"] == "hearing")
    assert hearing(installed=True, signed=True, speech="granted")["status"] == "granted"
    assert hearing(installed=True, signed=True, speech="restricted")["status"] == "denied"
    assert hearing(installed=True, signed=True, speech="not_determined")["status"] == "not_determined"
    unsigned = hearing(installed=True, signed=False)
    assert unsigned["status"] == "unknown" and "signature" in str(unsigned["blocked"])
    assert "FERB Voice" in str(hearing(installed=False)["blocked"])
    row = hearing(installed=True, signed=True, speech="denied")
    assert row["process"] == "voice" and row["pane"] == PANE + "Privacy_SpeechRecognition"
    assert row["requestable"] and not row["required"]
    assert all(r["id"] != "hearing" for r in P.rows({}, {}))


def test_clipboard_row_says_whether_macos_lets_ferb_read_what_you_copy() -> None:
    def row(state: str | None) -> dict[str, object]:
        return P.clipboard_row(state)
    assert row("allow")["status"] == row("unrestricted")["status"] == "granted"      # unrestricted: before 15.4
    assert row("default")["status"] == "not_determined" and row("default")["requestable"]   # macOS hasn't asked yet
    assert row("ask")["status"] == row("deny")["status"] == "denied"                  # the owner sets Allow in the pane
    assert not row("ask")["requestable"] and row(None)["status"] == "unknown"
    assert row("default")["pane"] == PANE + "Privacy_Pasteboard"
    assert row("default")["process"] == "daemon" and not row("default")["required"] and row("default")["blocked"] == ""


def test_app_panes_name_ferb_app_itself() -> None:
    assert P.app_panes("com.ferb.FERB") == {
        "notifications": "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=com.ferb.FERB",
        "microphone": PANE + "Privacy_Microphone", "speech_recognition": PANE + "Privacy_SpeechRecognition"}


def test_request_asks_macos_only_for_the_three_with_an_api(monkeypatch: pytest.MonkeyPatch) -> None:
    asked: list[tuple[str, object]] = []
    monkeypatch.setattr(P, "_ax_trusted", lambda prompt: asked.append(("ax", prompt)) or False)
    monkeypatch.setattr(P, "_request_screen_capture", lambda: asked.append(("screen", None)))
    monkeypatch.setattr(P, "_request_listen_event", lambda: asked.append(("hid", None)))
    monkeypatch.setattr(P, "screen_recording", lambda: P.Status.DENIED)
    monkeypatch.setattr(P, "input_monitoring", lambda: P.Status.GRANTED)
    assert P.request("accessibility") is P.Status.DENIED
    assert asked == [("ax", True), ("ax", False)]                 # the prompt, then a plain read
    assert P.request("screen_recording") is P.Status.DENIED
    assert P.request("input_monitoring") is P.Status.GRANTED
    assert asked[2:] == [("screen", None), ("hid", None)]
    for other in ("full_disk_access", "automation", "hearing", "clipboard", "nope"):
        with pytest.raises(ValueError):
            P.request(other)


def test_speech_request_is_ferb_voices_own_and_waits() -> None:
    app = Path("/Applications/FERB.app/Contents/Helpers/FERB Voice.app")
    assert P.speech_request_argv(app) == ["/usr/bin/open", "-n", "-W", str(app), "--args", "request-speech"]


def test_automation_candidates_are_the_running_scriptable_apps() -> None:
    apps = [AppInfo("com.apple.Notes", "Notes", "1", "/A/Notes.app", True),
            AppInfo("com.apple.Calculator", "Calculator", "1", "/A/Calculator.app", False),
            AppInfo("com.apple.mail", "Mail", "1", "/A/Mail.app", True)]
    assert P.automation_candidates(apps, {"com.apple.Notes", "com.apple.Calculator"}) == [("Notes", "com.apple.Notes")]


def test_grant_target_is_the_agent_app_or_the_checkout_interpreter(tmp_path: Path) -> None:
    assert P.grant_target(Path("/Applications/FERB.app"), "/x") == "/Applications/FERB.app/Contents/Helpers/FERB.app"
    real = tmp_path / "python3.12"
    real.write_text("")
    link = tmp_path / "python"
    link.symlink_to(real)
    assert P.grant_target(None, str(link)) == os.path.realpath(real)
```

- [ ] **Step 2: Run them to verify they fail**

Run: `nice -n 10 uv run pytest -q tests/test_permissions.py`
Expected: FAIL — `AttributeError: module 'ferb.device.permissions' has no attribute 'rows'` (and the other new names).

- [ ] **Step 3: Implement** in `ferb/device/permissions.py`

Change the imports at the top to:

```python
import asyncio
import ctypes
import os
import subprocess
import threading
from concurrent.futures import Future
from enum import Enum
from pathlib import Path
from typing import Any, Iterable

import ApplicationServices as AX
import Quartz
```

Replace `accessibility()` with a version that reads through the seam:

```python
def _ax_trusted(prompt: bool) -> bool:
    return bool(AX.AXIsProcessTrustedWithOptions({AX.kAXTrustedCheckOptionPrompt: prompt}))


def accessibility() -> Status:
    return Status.GRANTED if _ax_trusted(False) else Status.DENIED
```

Right after the existing `_iokit.IOHIDCheckAccess.restype = ctypes.c_uint32` line add:

```python
_iokit.IOHIDRequestAccess.argtypes = [ctypes.c_uint32]
_iokit.IOHIDRequestAccess.restype = ctypes.c_bool
```

Replace the `PANES` dict and `open_pane` with:

```python
PANES = {
    "accessibility": "Privacy_Accessibility",
    "screen_recording": "Privacy_ScreenCapture",
    "input_monitoring": "Privacy_ListenEvent",
    "full_disk_access": "Privacy_AllFiles",
    "automation": "Privacy_Automation",
    "microphone": "Privacy_Microphone",
    "speech_recognition": "Privacy_SpeechRecognition",
    "hearing": "Privacy_SpeechRecognition",          # FERB Voice's Speech Recognition (L26)
    "clipboard": "Privacy_Pasteboard",               # Paste from Other Apps (macOS 15.4+; I3)
}


def pane_url(permission: str) -> str:
    """The exact System Settings anchor for one Privacy & Security pane."""
    return f"x-apple.systempreferences:com.apple.preference.security?{PANES[permission]}"


def notifications_pane_url(bundle_id: str) -> str:
    """System Settings → Notifications → that app."""
    return f"x-apple.systempreferences:com.apple.Notifications-Settings.extension?id={bundle_id}"


def app_panes(bundle_id: str) -> dict[str, str]:
    """The panes for FERB.app's own rows (L34 C), which FERB.app reads and requests itself."""
    return {"notifications": notifications_pane_url(bundle_id), "microphone": pane_url("microphone"),
            "speech_recognition": pane_url("speech_recognition")}


def open_pane(permission: str) -> None:
    """Open System Settings on the right Privacy pane (FERB can't grant anything itself)."""
    subprocess.run(["open", pane_url(permission)], check=False)
```

Append at the end of the module:

```python
# ---------- L34 C: the setup window's rows (also `ferb setup`) ----------

REQUIRED = ("accessibility", "screen_recording", "full_disk_access")      # FERB can't act on the Mac without these
DAEMON_PERMISSIONS = (*REQUIRED, "input_monitoring")
REQUESTABLE = frozenset({"accessibility", "screen_recording", "input_monitoring", "clipboard", "hearing"})
# a new grant of these reaches a running process only after it restarts (Screen Recording: CGPreflight… stays false;
# Full Disk Access may need it too), so the window offers "Restart FERB to check" after a Grant
RESTART_TO_CHECK = frozenset({"screen_recording", "full_disk_access"})
FDA_WAITS = "FERB's shell sandbox and network proxy must be working first"


def fda_ready(containment: dict[str, Any]) -> bool:
    """CQ5: Full Disk Access only once containment exists: the shell's sandbox profile works, the shell runs inside it,
    and the egress proxy is there. `proxy_alive` counts when it was probed (`doctor.check`); the daemon's setup rows use
    the facts it started with, with no probe per poll."""
    return bool(containment.get("profile_ok") and containment.get("shell") and containment.get("proxy")
                and containment.get("proxy_alive", True))


def hearing_row_status(hearing: dict[str, Any]) -> tuple[str, str]:
    """(status, blocked) of FERB Voice's Speech Recognition from the daemon's hearing probe (`_hearing_status`)."""
    if not hearing.get("installed"):
        return Status.UNKNOWN.value, "FERB Voice isn't in this FERB.app; download FERB again"
    if not hearing.get("signed"):
        return Status.UNKNOWN.value, "FERB Voice's signature isn't the one setup recorded; set up FERB again"
    speech = str(hearing.get("speech") or "")
    if speech == "granted":
        return Status.GRANTED.value, ""
    if speech in ("denied", "restricted"):
        return Status.DENIED.value, ""
    if speech == "not_determined":
        return Status.NOT_DETERMINED.value, ""
    return Status.UNKNOWN.value, ""


def rows(grants: dict[str, str], containment: dict[str, Any],
         hearing: dict[str, Any] | None = None) -> list[dict[str, Any]]:
    """The daemon's own grants as setup rows: `process` "daemon" is FERB's agent app (the daemon's TCC identity);
    "voice" is FERB Voice, included only when its hearing probe was asked for."""
    ready = fda_ready(containment)
    out: list[dict[str, Any]] = [
        {"id": p, "status": grants.get(p, Status.UNKNOWN.value), "process": "daemon", "required": p in REQUIRED,
         "pane": pane_url(p), "requestable": p in REQUESTABLE, "restart_to_check": p in RESTART_TO_CHECK,
         "blocked": "" if p != "full_disk_access" or ready else FDA_WAITS}
        for p in DAEMON_PERMISSIONS]
    if hearing is not None:
        status, blocked = hearing_row_status(hearing)
        out.append({"id": "hearing", "status": status, "process": "voice", "required": False,
                    "pane": pane_url("hearing"), "requestable": True, "restart_to_check": False, "blocked": blocked})
    return out


def clipboard_row(state: str | None) -> dict[str, Any]:
    """I3 as an optional setup row (owner decision 2, 2026-10-25): whether macOS lets the daemon read the clipboard
    without asking, from the pasteboard's accessBehavior (never its contents). "default": macOS hasn't asked yet, so
    Grant lets it ask once (`setup.permission_request {permission: "clipboard"}`); "ask" and "deny" need FERB set to
    Allow in Paste from Other Apps; "unrestricted" is macOS before 15.4."""
    status = {"allow": Status.GRANTED.value, "unrestricted": Status.GRANTED.value,
              "default": Status.NOT_DETERMINED.value, "ask": Status.DENIED.value,
              "deny": Status.DENIED.value}.get(state or "", Status.UNKNOWN.value)
    return {"id": "clipboard", "status": status, "process": "daemon", "required": False, "pane": pane_url("clipboard"),
            "requestable": state == "default", "restart_to_check": False, "blocked": ""}


def _request_screen_capture() -> None:
    Quartz.CGRequestScreenCaptureAccess()


def _request_listen_event() -> None:
    _iokit.IOHIDRequestAccess(_LISTEN_EVENT)


def request(permission: str) -> Status:
    """Ask macOS for one grant on the owner's Grant (setup only): macOS shows its own prompt and lists FERB in that
    pane. Run in the daemon, so the grant attaches to the daemon's process. Full Disk Access has no request API (the
    owner adds FERB in the pane); Automation is per app (`automation(bundle_id, ask=True)`); hearing is FERB Voice's
    own request (`speech_request_argv`); the clipboard is one read by the daemon (`clipboard.request_access`)."""
    if permission == "accessibility":
        _ax_trusted(True)
        return accessibility()
    if permission == "screen_recording":
        _request_screen_capture()
        return screen_recording()
    if permission == "input_monitoring":
        _request_listen_event()
        return input_monitoring()
    raise ValueError(f"{permission!r} has no macOS request; add FERB in its System Settings pane")


def speech_request_argv(app: Path) -> list[str]:
    """FERB Voice asks macOS once for Speech Recognition (L26); `open -W` returns after the owner answered."""
    return ["/usr/bin/open", "-n", "-W", str(app), "--args", "request-speech"]


def automation_candidates(apps: Iterable[Any], running: set[str]) -> list[tuple[str, str]]:
    """(name, bundle id) of the scriptable apps running now: macOS can ask about an app only while it runs."""
    return [(a.name, a.bundle_id) for a in apps if a.scriptable and a.bundle_id in running]


def grant_target(bundle: Path | None, executable: str) -> str:
    """What the owner switches on in Privacy & Security for the daemon's grants: FERB's agent app inside FERB.app,
    or the real interpreter a checkout's daemon runs on."""
    from ferb.device.install import BUNDLE_AGENT_APP
    return str(bundle / BUNDLE_AGENT_APP) if bundle is not None else os.path.realpath(executable)
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `nice -n 10 uv run pytest -q tests/test_permissions.py`
Expected: PASS (the existing tests too: `check_all` still returns the four keys; `PANES` still holds the five old
keys).

- [ ] **Step 5: Commit**

```bash
git add ferb/device/permissions.py tests/test_permissions.py
git commit -m "feat(L34): permission rows, macOS requests and shared setup facts for FERB.app's setup window (CQ5)"
```

---

### Task 2: The daemon's permission RPCs, Done, and `ferb setup` on the same functions

**Files:**
- Modify: `ferb/daemon/main.py` (Daemon `__init__` ~`:436`; RPC registration ~`:501-535`; new methods near
  `_automation_request` ~`:4941`)
- Modify: `ferb/daemon/auth.py` (`METHODS[Role.CLI]`)
- Modify: `ferb/daemon/server.py` (`LONG_WAITS`)
- Modify: `ferb/cli/setup.py` (`REQUIRED_PERMISSIONS`, step 3's containment check, `_grant`, `_automation`,
  `hearing_setup`)
- Modify: `docs/RPC.md` (new section, first rows)
- Create: `tests/test_setup_rpcs.py`
- Modify: `tests/test_setup_flow.py` (`FakeDaemon`, `FakeClient`, one new test)

**Interfaces:**
- Consumes: Task 1's `permissions.rows`, `request`, `REQUESTABLE`, `hearing_row_status`, `speech_request_argv`,
  `automation_candidates`, `grant_target`, `app_panes`, `pane_url`, `fda_ready`, `REQUIRED`.
- Produces:
  - RPCs `setup.permissions`, `setup.permission_request`, `setup.finish` (shapes in "Interfaces for other phases").
  - Daemon seams tests replace: `Daemon.permission_check: Callable[[], dict[str, str]]`,
    `Daemon.permission_request: Callable[[str], permissions.Status]`,
    `Daemon.automation_check: Callable[[str], Awaitable[permissions.Status]]`,
    `Daemon.speech_request: Callable[[Path], Any]`, `Daemon.clipboard_request: Callable[[], str | None]`,
    `Daemon._setup_containment() -> dict[str, Any]` (tests also replace the existing `Daemon._clipboard_access`).
  - Journal kinds `setup.permission_requested {permission, status}` and `setup.finished {by: "app"}`.

- [ ] **Step 1: Write the failing daemon tests** (`tests/test_setup_rpcs.py`)

```python
"""L34 C: the setup window's permission RPCs. Rows are the daemon's own grants with their exact panes; the owner's Grant
becomes a macOS request made by the daemon; Done is recorded as `ferb setup` records it. Nothing here prompts: every
macOS call is replaced."""
import asyncio
from typing import Any, Callable

import pytest

from ferb.cli.client import Client, RpcError
from ferb.device import permissions as P
from ferb.device.profile import AppInfo, DeviceProfile
from tests.conftest import RunningDaemon


def call(daemon: RunningDaemon, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
    client = Client(daemon.sock)
    try:
        return client.call(method, params or {})
    finally:
        client.close()


def on_loop(daemon: RunningDaemon, fn: Callable[[], Any]) -> Any:
    async def go() -> Any:
        return fn()
    return daemon.run(go())


@pytest.fixture
def grants(daemon: RunningDaemon) -> dict[str, str]:
    """macOS as the daemon reads it, faked; a request grants the permission, as the owner would in System Settings."""
    state = {"accessibility": "denied", "screen_recording": "granted", "input_monitoring": "not_determined",
             "full_disk_access": "denied"}
    d = daemon.daemon

    def requested(permission: str) -> P.Status:
        state[permission] = "granted"
        return P.Status.GRANTED
    d.permission_check = lambda: dict(state)                   # type: ignore[method-assign]
    d.permission_request = requested                           # type: ignore[method-assign]
    d._setup_containment = lambda: {"profile_ok": True, "shell": True, "proxy": True}   # type: ignore[method-assign]
    d._device_profile = lambda: None                           # type: ignore[method-assign]
    d._clipboard_access = lambda: "default"                    # type: ignore[method-assign]   # macOS hasn't asked
    return state


def notes_profile() -> DeviceProfile:
    return DeviceProfile("dev", "macOS", "15.0", "2026-10-25T10:00:00", [
        AppInfo("com.apple.Notes", "Notes", "1", "/A/Notes.app", True),
        AppInfo("com.apple.Calculator", "Calculator", "1", "/A/Calculator.app", False),
        AppInfo("com.apple.mail", "Mail", "1", "/A/Mail.app", True)], [])


def test_permission_rows_are_the_daemons_own_with_their_panes(daemon: RunningDaemon, grants: dict[str, str]) -> None:
    reply = call(daemon, "setup.permissions")
    rows = {r["id"]: r for r in reply["rows"]}
    assert list(rows) == ["accessibility", "screen_recording", "full_disk_access", "input_monitoring", "clipboard"]
    assert rows["accessibility"]["status"] == "denied" and rows["screen_recording"]["status"] == "granted"
    assert rows["clipboard"]["status"] == "not_determined" and rows["clipboard"]["pane"].endswith("?Privacy_Pasteboard")
    assert rows["accessibility"]["pane"].endswith("?Privacy_Accessibility")
    assert reply["app_panes"]["notifications"].endswith("?id=com.ferb.FERB")
    assert reply["automation_pane"].endswith("?Privacy_Automation")
    assert reply["grant_target"] and reply["automation"] == [] and reply["setup_done"] is False


def test_full_disk_access_row_waits_for_the_containment_the_daemon_started_with(
        daemon: RunningDaemon, grants: dict[str, str]) -> None:
    daemon.daemon._setup_containment = lambda: {"profile_ok": False, "shell": False, "proxy": True}   # type: ignore[method-assign]
    fda = next(r for r in call(daemon, "setup.permissions")["rows"] if r["id"] == "full_disk_access")
    assert fda["blocked"] == P.FDA_WAITS


def test_automation_lists_running_scriptable_apps_with_their_state(daemon: RunningDaemon, grants: dict[str, str]) -> None:
    d = daemon.daemon
    d._device_profile = notes_profile                         # type: ignore[method-assign]
    d._running_apps = lambda: [("com.apple.Notes", "Notes", 11), ("com.apple.Calculator", "Calculator", 12)]  # type: ignore[method-assign]

    async def status(bundle: str) -> P.Status:
        return P.Status.NOT_DETERMINED
    d.automation_check = status                                # type: ignore[method-assign]
    assert call(daemon, "setup.permissions")["automation"] == [
        {"name": "Notes", "bundle_id": "com.apple.Notes", "status": "not_determined"}]


def test_a_hung_automation_check_reads_unknown_within_the_reply(daemon: RunningDaemon, grants: dict[str, str]) -> None:
    d = daemon.daemon
    d._device_profile = notes_profile                         # type: ignore[method-assign]
    d._running_apps = lambda: [("com.apple.Notes", "Notes", 11)]   # type: ignore[method-assign]

    async def hang(bundle: str) -> P.Status:
        await asyncio.sleep(30)
        return P.Status.GRANTED
    d.automation_check = hang                                  # type: ignore[method-assign]
    assert call(daemon, "setup.permissions")["automation"][0]["status"] == "unknown"


def test_hearing_is_probed_only_when_asked(daemon: RunningDaemon, grants: dict[str, str]) -> None:
    async def probe(asked: bool) -> dict[str, Any]:
        return {"installed": True, "signed": True, "speech": "not_determined"} if asked else {"installed": True}
    daemon.daemon._hearing_status = probe                      # type: ignore[method-assign]
    assert all(r["id"] != "hearing" for r in call(daemon, "setup.permissions")["rows"])
    hearing = next(r for r in call(daemon, "setup.permissions", {"hearing": True})["rows"] if r["id"] == "hearing")
    assert hearing["status"] == "not_determined" and hearing["process"] == "voice"


def test_grant_asks_macos_in_the_daemon_and_journals_it(daemon: RunningDaemon, grants: dict[str, str]) -> None:
    assert call(daemon, "setup.permission_request", {"permission": "accessibility"}) == {
        "permission": "accessibility", "status": "granted"}
    entries = on_loop(daemon, lambda: daemon.daemon._journal().of_kind("setup.permission_requested"))
    assert [e.data for e in entries] == [{"permission": "accessibility", "status": "granted"}]
    for refused in ("full_disk_access", "automation", "microphone", ""):
        with pytest.raises(RpcError):
            call(daemon, "setup.permission_request", {"permission": refused})


def test_clipboard_grant_lets_macos_ask_once_and_journals_what_it_answered(daemon: RunningDaemon,
                                                                          grants: dict[str, str]) -> None:
    daemon.daemon.clipboard_request = lambda: "ask"            # type: ignore[method-assign]   # macOS: ask each time
    assert call(daemon, "setup.permission_request", {"permission": "clipboard"}) == {
        "permission": "clipboard", "status": "denied"}         # the owner still has to set Allow in the pane
    access = on_loop(daemon, lambda: daemon.daemon._journal().of_kind("clipboard.access"))
    assert [e.data for e in access] == [{"access": "ask", "requested": True}]


def test_hearing_grant_runs_ferb_voices_own_request(daemon: RunningDaemon, grants: dict[str, str]) -> None:
    ran: list[Any] = []
    d = daemon.daemon
    d.speech_request = ran.append                              # type: ignore[method-assign]

    async def probe(asked: bool) -> dict[str, Any]:
        return {"installed": True, "signed": True, "speech": "granted"}
    d._hearing_status = probe                                  # type: ignore[method-assign]
    assert call(daemon, "setup.permission_request", {"permission": "hearing"})["status"] == "granted"
    assert ran == [d.voice_bundle]


def test_setup_finish_and_cli_setup_both_mark_setup_done(daemon: RunningDaemon, grants: dict[str, str]) -> None:
    assert call(daemon, "setup.permissions")["setup_done"] is False
    assert call(daemon, "setup.finish") == {"setup_done": True}
    assert daemon.daemon.config.get("setup", "permissions_version") == 27
    assert call(daemon, "setup.permissions")["setup_done"] is True
    daemon.daemon.config.set("setup", "permissions_version", 31)          # a later `ferb setup` never goes backwards
    call(daemon, "setup.finish")
    assert daemon.daemon.config.get("setup", "permissions_version") == 31
```

- [ ] **Step 2: Write the failing CLI test** in `tests/test_setup_flow.py`

In `FakeDaemon.__init__` add after `self.automation_requests …`:

```python
        self.requested: list[str] = []   # setup.permission_request (L34 C): macOS asked to list FERB in that pane
```

In `FakeClient.call`, before the final `raise AssertionError(method)`, add:

```python
        if method == "setup.permission_request":     # macOS shows its prompt; granting stays the owner's (in Settings)
            self.d.requested.append(params["permission"])
            return {"permission": params["permission"], "status": self.d.perms.get(params["permission"], "unknown")}
```

Append the test after `test_input_monitoring_is_collected_at_setup`:

```python
def test_setup_asks_macos_to_list_ferb_before_opening_a_pane(world: dict[str, Any]) -> None:
    env, out = make_env(world, ["", "", "", ""], daemon_up=False)
    assert S.setup(env) == 0, out.getvalue()
    # Full Disk Access has no request; Input Monitoring was already granted
    assert world["daemon"].requested == ["accessibility", "screen_recording"]
```

- [ ] **Step 3: Run them to verify they fail**

Run: `nice -n 10 uv run pytest -q tests/test_setup_rpcs.py tests/test_setup_flow.py -k "permission or setup_done or hearing or list_ferb"`
Expected: FAIL — `RpcError: … not allowed` / `no method setup.permissions`, and `requested == []` in the CLI test.

- [ ] **Step 4: Implement the daemon side**

In `ferb/daemon/auth.py`, add to the `Role.CLI` frozenset (after `"reports.build"`):

```python
                         "setup.permissions", "setup.permission_request", "setup.finish",
```

In `ferb/daemon/server.py` replace `LONG_WAITS` with:

```python
# Calls the notch waits longer for (seconds; its `wait:` calls): a failed task's report renders and redacts the task's
# whole record first (L35; NotchModel.reportWait); a permission request and an Automation request wait for the owner's
# answer to macOS's prompt, and a runtime check makes one model call (L34 C, the setup window)
LONG_WAITS = {"reports.build": 120.0, "setup.permission_request": 120.0, "automation.request": 120.0,
              "runtimes.check": 60.0}
```

In `ferb/daemon/main.py`, at the end of `Daemon.__init__` (right after the `self.notify: … = …` line) add:

```python
        # L34 C: the setup window's permission checks and requests; tests replace them (nothing there may prompt)
        from ferb.device import permissions as _permissions
        self.permission_check: Callable[[], dict[str, str]] = _permissions.check_all
        self.permission_request: Callable[[str], Any] = _permissions.request
        self.automation_check: Callable[[str], Awaitable[Any]] = (
            lambda bundle: _permissions.automation_async(bundle, ask=False))
        self.speech_request: Callable[[Path], Any] = (
            lambda app: subprocess.run(_permissions.speech_request_argv(app), check=False))

        def _clipboard_request() -> Any:          # one read, so macOS shows its paste alert (I3); nothing kept
            from ferb.sense.watchers.clipboard import request_access
            return request_access()
        self.clipboard_request: Callable[[], Any] = _clipboard_request
```

In the RPC registration block, after `self.server.unary['reports.build'] = self._reports_build` add:

```python
        self.server.unary['setup.permissions'] = self._setup_permissions
        self.server.unary['setup.permission_request'] = self._setup_permission_request
        self.server.unary['setup.finish'] = self._setup_finish
```

Add these methods right after `_automation_request`:

```python
    def _setup_containment(self) -> dict[str, Any]:
        """The containment facts this daemon started with (CQ5) for the Full Disk Access row: no sandbox-exec run or proxy
        probe per poll; `doctor.check` is the live check."""
        return {"profile_ok": bool(self.containment[0]), "shell": "shell.run" in self.registry.capabilities,
                "proxy": self.proxy is not None}

    async def _setup_permissions(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C: the setup window's Permissions step (and Settings → Permissions): each grant as this daemon's own
        process holds it, never a prompt. `hearing: true` also probes FERB Voice's Speech Recognition (it starts FERB
        Voice in the background), so the window asks for it only on entering the step and after a request."""
        from ferb.device import permissions
        from ferb.device.install import APP_BUNDLE_ID
        from ferb.paths import app_bundle
        grants = await asyncio.to_thread(self.permission_check)
        hearing = await self._hearing_status(True) if params.get("hearing") is True else None
        profile = self._device_profile()
        running = {bundle for bundle, _, _ in self._running_apps()}
        apps = permissions.automation_candidates(profile.apps if profile is not None else [], running)

        async def automation(bundle: str) -> str:
            try:
                return str((await asyncio.wait_for(self.automation_check(bundle), 2)).value)
            except TimeoutError:
                return permissions.Status.UNKNOWN.value
        statuses = await asyncio.gather(*(automation(bundle) for _, bundle in apps))
        clipboard = permissions.clipboard_row(await asyncio.to_thread(self._clipboard_access))   # owner decision 2
        return {"rows": permissions.rows(grants, self._setup_containment(), hearing) + [clipboard],
                "automation": [{"name": name, "bundle_id": bundle, "status": status}
                               for (name, bundle), status in zip(apps, statuses)],
                "automation_pane": permissions.pane_url("automation"),
                "grant_target": permissions.grant_target(app_bundle(), sys.executable),
                "app_panes": permissions.app_panes(APP_BUNDLE_ID),
                "setup_done": int(self.config.get("setup", "permissions_version", 0) or 0) >= 27}

    async def _setup_permission_request(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C: the owner's Grant on one row: macOS's own prompt, which also lists FERB in that pane; made here so the
        grant attaches to this daemon's process. Hearing is FERB Voice asking for itself, and returns after the owner
        answered. Journaled."""
        from ferb.device import permissions
        permission = str(params.get("permission", ""))
        if permission not in permissions.REQUESTABLE:
            raise FerbError(f"{permission or 'That permission'} has no request; add FERB in its System Settings pane")
        if permission == "hearing":
            await asyncio.to_thread(self.speech_request, self.voice_bundle)
            status, _ = permissions.hearing_row_status(await self._hearing_status(True))
        elif permission == "clipboard":           # as `clipboard.request`: one read, journaled; nothing read is kept
            access = await asyncio.to_thread(self.clipboard_request)
            self._journal().append("clipboard.access", {"access": access, "requested": True})
            status = str(permissions.clipboard_row(access)["status"])
        else:
            status = str((await asyncio.to_thread(self.permission_request, permission)).value)
        self._journal().append("setup.permission_requested", {"permission": permission, "status": status})
        return {"permission": permission, "status": status}

    async def _setup_finish(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C: the setup window's Done, recorded as `ferb setup` records the end of its permission step, so launch
        stops showing the window."""
        current = int(self.config.get("setup", "permissions_version", 0) or 0)
        self.config.set("setup", "permissions_version", max(27, current))
        self._journal().append("setup.finished", {"by": "app"})
        return {"setup_done": True}
```

- [ ] **Step 5: Implement the CLI side** in `ferb/cli/setup.py`

Replace the two constants near the top:

```python
REQUIRED_PERMISSIONS = permissions.REQUIRED
SETUP_PERMISSIONS = permissions.DAEMON_PERMISSIONS
```

In `setup()`, step 3's Full Disk Access check becomes:

```python
        if perm == "full_disk_access":
            containment = client.call("doctor.check", {"selftest": False}).get("containment", {})
            if not permissions.fda_ready(containment):
                env.say("Full Disk Access setup deferred: containment must report a working shell profile and proxy first.")
                continue
            env.say("Full Disk Access is part of FERB's full-access setup; containment is active before this grant.")
```

In `_grant`, ask macOS to list FERB once before the first pane opens:

```python
def _grant(env: Env, client: Client, perm: str, binary: Path | None, required: bool) -> Client:
    label = perm.replace("_", " ").title()
    requested = False
    while _status(client, perm) != "granted":
        if not requested and perm in permissions.REQUESTABLE:     # macOS lists FERB in that pane (L34 C)
            requested = True
            try:
                client.call("setup.permission_request", {"permission": perm})
            except FerbError:
                pass                                              # an older daemon: the pane still opens
        if env.inst.bundle:
```

(keep the rest of the loop body unchanged).

In `_automation`, replace everything from `running = env.running_apps()` through the `chosen = [...]` assignment
(the `if chosen:` block after it stays) with:

```python
    running = env.running_apps()
    candidates = permissions.automation_candidates(profile.apps if profile else [], running)
    if not candidates:
        env.say("     No scriptable apps are running. Rerun setup to grant Automation for additional apps.")
        return
    for i, (name, bundle) in enumerate(candidates, 1):
        env.say(f"     {i:>2}. {name} ({bundle})")
    picked = env.ask("     Numbers to request now, comma-separated (Enter for none): ").strip()
    chosen = [candidates[int(n) - 1][1] for n in picked.replace(" ", "").split(",") if n.isdigit()
              and 1 <= int(n) <= len(candidates)]
```

In `hearing_setup`, replace the `env.run([...request-speech])` line with:

```python
    env.run(permissions.speech_request_argv(app))
```

- [ ] **Step 6: Document the RPCs** — append to `docs/RPC.md`:

```markdown
## FERB.app's setup window and Settings (L34)

FERB.app's native setup window (Phase C) and its Settings pages (Phase E) read and act through these CLI-role methods;
`ferb setup` uses the same functions (`ferb/device/permissions.py`, `ferb/device/extensions.py`, `ferb/cli/runtimes.py`).
None of them prompts unless its row says so.

| Method | Parameters | Result / behavior |
|---|---|---|
| `setup.permissions` | `{hearing?: bool}` | `{rows, automation, automation_pane, grant_target, app_panes, setup_done}`. Each row: `{id, status, process, required, pane, requestable, restart_to_check, blocked}` for `accessibility`, `screen_recording`, `full_disk_access`, `input_monitoring`, `clipboard` (Paste from Other Apps, from the pasteboard's accessBehavior; process `daemon`: FERB's agent app, the daemon's own TCC identity), plus `hearing` (process `voice`: FERB Voice's Speech Recognition) only with `hearing: true`, which starts FERB Voice in the background to ask it. `status`: `granted`, `denied`, `not_determined`, `target_not_running` or `unknown`; `blocked`: why a row can't be granted yet (Full Disk Access until containment works, CQ5). `automation`: `[{name, bundle_id, status}]` for the running scriptable apps (each check waits at most 2 s, then `unknown`). `grant_target`: the path macOS lists as FERB for the daemon's grants. `app_panes`: System Settings URLs for FERB.app's own rows (`notifications`, `microphone`, `speech_recognition`). `setup_done`: `[setup] permissions_version` ≥ 27. Never a prompt |
| `setup.permission_request` | `{permission: "accessibility"\|"screen_recording"\|"input_monitoring"\|"clipboard"\|"hearing"}` | `{permission, status}`: macOS's own prompt, made by the daemon so the grant attaches to it (it also lists FERB in that pane); `clipboard` reads the clipboard once so macOS shows its paste alert (journaled `clipboard.access`, nothing read is kept); `hearing` runs FERB Voice's one Speech Recognition request and returns after the owner answered. Journaled `setup.permission_requested`. Any other permission is refused (Full Disk Access has no request; Automation is `automation.request`) |
| `setup.finish` | `{}` | `{setup_done: true}`; records `[setup] permissions_version = max(27, current)`; journaled `setup.finished` |
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `nice -n 10 uv run pytest -q tests/test_setup_rpcs.py tests/test_setup_flow.py tests/test_permissions.py tests/test_auth.py tests/test_failure_report.py`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add ferb/daemon/main.py ferb/daemon/auth.py ferb/daemon/server.py ferb/cli/setup.py docs/RPC.md \
  tests/test_setup_rpcs.py tests/test_setup_flow.py
git commit -m "feat(L34): setup.permissions, setup.permission_request and setup.finish; ferb setup asks macOS to list FERB first (CQ5)"
```

---

### Task 3: Choose your AI: `runtimes.status`, `runtimes.use`, and a redacted `runtimes.check`

**Files:**
- Modify: `ferb/cli/runtimes.py`
- Modify: `ferb/daemon/main.py` (registration; new methods next to `_runtimes_check` ~`:4432`; `_runtimes_check`)
- Modify: `ferb/daemon/auth.py`
- Modify: `docs/RPC.md` (two rows in the L34 section)
- Test: `tests/test_runtime_selection.py`

**Interfaces:**
- Produces:
  - `ferb.cli.runtimes.roles(cfg: Config) -> dict[str, str]`
  - `ferb.cli.runtimes.status(home: Path) -> dict[str, Any]` — `{"roles": {...}, "runtimes": [{"name", "usable", "fix", "model"}]}`
  - `ferb.cli.runtimes.assign(home: Path, runtime: str, chosen: Iterable[str] = ROLES, model: str | None = None) -> dict[str, str]` (raises `ValueError`)
  - RPCs `runtimes.status`, `runtimes.use`; journal kind `runtimes.changed {runtime, roles}`.

- [ ] **Step 1: Write the failing tests** (append to `tests/test_runtime_selection.py`)

```python
# ---------- L34 C: Choose your AI ----------

import pytest

from ferb.cli.client import Client, RpcError


def test_status_lists_each_runtime_and_what_each_role_uses(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("ferb.runtimes.keys.has_api_key", lambda provider: provider == "openai")
    monkeypatch.setattr("ferb.runtimes.codex_exec.codex_binary", lambda *a, **k: Path("/x/codex"))
    out = runtimes_cli.status(tmp_path)
    rows = {r["name"]: r for r in out["runtimes"]}
    assert out["roles"] == {"orchestrator": "codex", "worker": "codex", "checker": "codex"}
    assert set(rows) == set(CATALOG)
    assert rows["codex"]["usable"] is False and "codex-login" in rows["codex"]["fix"]   # FERB's own Codex not signed in
    assert rows["openai"]["usable"] is True and rows["anthropic"]["usable"] is False
    assert rows["claude"]["usable"] is True and rows["codex"]["model"] == CATALOG["codex"].default_model
    (tmp_path / "codex-home").mkdir()
    (tmp_path / "codex-home" / "auth.json").write_text("{}")
    assert {r["name"]: r for r in runtimes_cli.status(tmp_path)["runtimes"]}["codex"]["usable"] is True


def test_assign_points_the_roles_at_one_runtime_or_names_what_is_unknown(tmp_path: Path) -> None:
    assert runtimes_cli.assign(tmp_path, "claude") == {"orchestrator": "claude", "worker": "claude", "checker": "claude"}
    assert runtimes_cli.assign(tmp_path, "codex", ["worker"]) == {"orchestrator": "claude", "worker": "codex",
                                                                   "checker": "claude"}
    with pytest.raises(ValueError, match="Unknown runtime"):
        runtimes_cli.assign(tmp_path, "gpt-9")
    with pytest.raises(ValueError, match="Unknown role"):
        runtimes_cli.assign(tmp_path, "codex", ["boss"])
    with pytest.raises(ValueError, match="at least one role"):
        runtimes_cli.assign(tmp_path, "codex", [])


def test_ferb_runtimes_use_still_says_what_it_did(tmp_path: Path) -> None:
    out = io.StringIO()
    assert runtimes_cli.use(tmp_path, "worker", "claude", None, out) == 0
    assert "The worker now uses claude from its next call." in out.getvalue()
    bad = io.StringIO()
    assert runtimes_cli.use(tmp_path, "boss", "claude", None, bad) == 1 and "Unknown role 'boss'" in bad.getvalue()


def test_choose_your_ai_switches_every_role_and_journals_it(daemon: RunningDaemon,
                                                            monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("ferb.runtimes.keys.has_api_key", lambda provider: False)
    client = Client(daemon.sock)
    try:
        reply = client.call("runtimes.use", {"runtime": "anthropic"})
        status = client.call("runtimes.status")
        with pytest.raises(RpcError, match="Unknown runtime"):
            client.call("runtimes.use", {"runtime": "nope"})
        with pytest.raises(RpcError, match="roles must be a list"):
            client.call("runtimes.use", {"runtime": "codex", "roles": "worker"})
    finally:
        client.close()
    assert reply == {"roles": {"orchestrator": "anthropic", "worker": "anthropic", "checker": "anthropic"},
                     "usable": False, "fix": "run `ferb setup --key anthropic`"}
    assert status["roles"]["worker"] == "anthropic"
    assert daemon.daemon._runtime_for("orchestrator") == "anthropic"


def test_a_runtime_checks_failure_detail_is_redacted(daemon: RunningDaemon) -> None:
    d = daemon.daemon
    daemon.call_soon(d._add_secret_value, "sk-proj-secretvalue0123456789")

    def boom(*args: object, **kwargs: object) -> object:
        raise RuntimeError("Incorrect API key provided: sk-proj-secretvalue0123456789")
    d._build_runtime = boom                                    # type: ignore[method-assign]
    client = Client(daemon.sock)
    try:
        reply = client.call("runtimes.check", {"runtime": "openai"})
    finally:
        client.close()
    assert reply["ok"] is False and "sk-proj-secretvalue0123456789" not in reply["detail"]
```

- [ ] **Step 2: Run them to verify they fail**

Run: `nice -n 10 uv run pytest -q tests/test_runtime_selection.py`
Expected: FAIL — `AttributeError: module 'ferb.cli.runtimes' has no attribute 'status'`, the RPC tests with
`not allowed`, and the redaction test showing the key in `detail`.

- [ ] **Step 3: Implement `ferb/cli/runtimes.py`**

Replace the module body below the docstring with:

```python
from pathlib import Path
from typing import Any, Iterable, TextIO

from ferb.config import Config
from ferb.runtimes.catalog import CATALOG, ROLES


def roles(cfg: Config) -> dict[str, str]:
    """Which runtime each role uses; the checker follows the worker unless set (D32)."""
    worker = str(cfg.get("runtimes", "worker", "codex"))
    return {"orchestrator": str(cfg.get("runtimes", "orchestrator", "codex")), "worker": worker,
            "checker": str(cfg.get("runtimes", "checker", worker))}


def status(home: Path) -> dict[str, Any]:
    """Each runtime with whether it can run on this Mac (and the fix when not), and what each role uses: the setup
    window's Choose your AI (L34 C) and `ferb runtimes`. Presence checks only: no key is read."""
    cfg = Config(home)
    rows = []
    for name, info in CATALOG.items():
        ok, fix = info.available(home)
        rows.append({"name": name, "usable": ok, "fix": fix,
                     "model": str(cfg.get("models", name) or info.default_model or "")})
    return {"roles": roles(cfg), "runtimes": rows}


def assign(home: Path, runtime: str, chosen: Iterable[str] = ROLES, model: str | None = None) -> dict[str, str]:
    """Point the chosen roles at `runtime` (and its model, when given) from their next call. ValueError names an
    unknown role or runtime; nothing is written then."""
    chosen = list(chosen)
    if not chosen:
        raise ValueError(f"Name at least one role; roles: {', '.join(ROLES)}.")
    unknown = [role for role in chosen if role not in ROLES]
    if unknown:
        raise ValueError(f"Unknown role {unknown[0]!r}; roles: {', '.join(ROLES)}.")
    if runtime not in CATALOG:
        raise ValueError(f"Unknown runtime {runtime!r}; runtimes: {', '.join(CATALOG)}.")
    cfg = Config(home)
    for role in chosen:
        cfg.set("runtimes", role, runtime)
    if model:
        cfg.set("models", runtime, model)
    return roles(cfg)


def show(home: Path, out: TextIO) -> int:
    cfg = Config(home)
    print(f"{'runtime':<10}{'usable':<8}{'model':<20}{'tools':<9}{'cache':<9}{'cache key':<11}{'typed':<8}{'effort':<8}"
          f"{'images':<8}billing", file=out)
    for name, info in CATALOG.items():
        model = str(cfg.get("models", name) or info.default_model or "(default)")
        ok, fix = info.available(home)
        c = info.capabilities(model)
        print(f"{name:<10}{'yes' if ok else 'no':<8}{model:<20}{c.tools:<9}{c.cache:<9}{'yes' if c.cache_key else 'no':<11}"
              f"{c.structured_output:<8}{'yes' if c.reasoning_effort else 'no':<8}{'yes' if c.image_input else 'no':<8}"
              f"{c.billing}" + (f"   ({fix})" if fix else ""), file=out)
    uses = roles(cfg)
    print("\n" + "\n".join(f"{role}: {uses[role]}" for role in ROLES), file=out)
    return 0


def use(home: Path, role: str, runtime: str, model: str | None, out: TextIO) -> int:
    try:
        assign(home, runtime, [role], model)
    except ValueError as e:
        print(str(e), file=out)
        return 1
    print(f"The {role} now uses {runtime}" + (f" with {model}" if model else "") + " from its next call.", file=out)
    ok, fix = CATALOG[runtime].available(home)
    if not ok:
        print(f"{runtime} isn't usable on this Mac yet: {fix}. The {role} will fail until then.", file=out)
    return 0
```

- [ ] **Step 4: Implement the daemon side**

`ferb/daemon/auth.py`: add `"runtimes.status", "runtimes.use",` to the `Role.CLI` set.

`ferb/daemon/main.py` registration (after the Task 2 lines):

```python
        self.server.unary['runtimes.status'] = self._runtimes_status
        self.server.unary['runtimes.use'] = self._runtimes_use
```

Add before `_runtimes_check`:

```python
    async def _runtimes_status(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C: Choose your AI's live status (and Settings → AI): each runtime's availability, each role's runtime."""
        from ferb.cli import runtimes as runtimes_cli
        return await asyncio.to_thread(runtimes_cli.status, self.home)

    async def _runtimes_use(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C: the owner's choice of AI service for the roles named (all three by default), from its next call;
        journaled. One that can't run here yet is set anyway and says so, as `ferb runtimes use` does."""
        from ferb.cli import runtimes as runtimes_cli
        from ferb.runtimes.catalog import CATALOG, ROLES
        runtime = str(params.get("runtime", ""))
        chosen = params.get("roles", list(ROLES))
        if not isinstance(chosen, list) or not all(isinstance(role, str) for role in chosen):
            raise FerbError("roles must be a list of role names")
        try:
            assigned = runtimes_cli.assign(self.home, runtime, chosen)
        except ValueError as e:
            raise FerbError(str(e)) from None
        self._available_cache.pop(runtime, None)
        self._journal().append("runtimes.changed", {"runtime": runtime, "roles": chosen})
        usable, fix = await asyncio.to_thread(CATALOG[runtime].available, self.home)
        return {"roles": assigned, "usable": usable, "fix": fix}
```

In `_runtimes_check`, redact both failure details (rule 14: a provider's error can quote the key):

```python
        except Exception as e:
            return {"ok": False, "detail": self._redact_text(f"{type(e).__name__}: {e}")}
```

and inside the event loop:

```python
                if ev.kind in ("error", "violation"):
                    detail = self._redact_text(str(ev.data))
```

- [ ] **Step 5: Document** — add to the `docs/RPC.md` L34 table:

```markdown
| `runtimes.status` | `{}` | `{roles: {orchestrator, worker, checker}, runtimes: [{name, usable, fix, model}]}`; presence checks only (no key is read) |
| `runtimes.use` | `{runtime, roles?: [role,...]}` | Points the roles (default all three) at `runtime` from their next call: `{roles, usable, fix}`; an unknown role or runtime is refused; journaled `runtimes.changed`. `runtimes.check`'s `detail` is redacted |
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `nice -n 10 uv run pytest -q tests/test_runtime_selection.py tests/test_cli.py`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add ferb/cli/runtimes.py ferb/daemon/main.py ferb/daemon/auth.py docs/RPC.md tests/test_runtime_selection.py
git commit -m "feat(L34): runtimes.status and runtimes.use for Choose your AI; runtimes.check's detail is redacted (D32)"
```

---

### Task 4: Browsers: read-only states, setup rows, and Add to Chrome

**Files:**
- Modify: `ferb/act/browser/autoinstall.py` (new `read_states`)
- Modify: `ferb/device/extensions.py` (new `store_page`, `profile_state`, `browser_rows`)
- Modify: `ferb/daemon/main.py` (registration; new methods after `_browsers_unpair` ~`:4240`)
- Modify: `ferb/daemon/auth.py`
- Modify: `docs/RPC.md`
- Test: `tests/test_extension_autoinstall.py`

**Interfaces:**
- Produces:
  - `autoinstall.read_states(support: Path, store_id: str, ids: tuple[str, ...], paired: set[tuple[str, str]] | None, data_dirs: dict[str, Path] | None = None) -> list[BrowserState]` — writes nothing.
  - `extensions.store_page(store_id: str) -> str` (`""` without a valid id)
  - `extensions.profile_state(p: ProfileStatus) -> str` ∈ `not_installed`, `removed`, `installed`, `on`, `paired`
  - `extensions.browser_rows(states, store_id, extension_dir, ferb_identity: tuple[str, str] | None, connected: set[tuple[str, str]]) -> dict[str, Any]`
  - RPCs `setup.browsers`, `setup.browser_open`; journal kind `setup.browser_opened {browser, folder, url}`.

- [ ] **Step 1: Write the failing tests** (append to `tests/test_extension_autoinstall.py`)

```python
# ---------- L34 C: the setup window's Browsers step ----------

from ferb.cli.client import Client, RpcError  # noqa: E402
from ferb.device.extensions import browser_rows  # noqa: E402
from tests.conftest import RunningDaemon  # noqa: E402


def test_read_states_reports_what_prepare_does_and_writes_nothing(world: dict[str, Any]) -> None:
    before = sorted(world["support"].rglob("*"))
    states = autoinstall.read_states(world["support"], STORE, (PINNED, STORE), {("brave", "Default")})
    assert sorted(world["support"].rglob("*")) == before                    # no request file, no folder
    by = {(s.browser, p.folder): p for s in states for p in s.profiles}
    assert by[("chrome", "Default")].enabled and by[("brave", "Default")].paired
    assert all(s.request is None for s in states)                         # nothing requested yet
    written = autoinstall.prepare(world["support"], STORE, (PINNED, STORE), None)
    again = autoinstall.read_states(world["support"], STORE, (PINNED, STORE), None)
    assert {s.browser for s in again if s.request} == {s.browser for s in written if s.request}


def test_setup_rows_name_each_profiles_state_and_where_load_unpacked_is_the_way(world: dict[str, Any]) -> None:
    autoinstall.prepare(world["support"], STORE, (PINNED, STORE), None)
    states = autoinstall.read_states(world["support"], STORE, (PINNED, STORE), {("brave", "Default")})
    rows = browser_rows(states, STORE, Path("/x/chromium"), ("chrome", "Profile 1"), {("brave", "Default")})
    assert rows["store_url"] == f"https://chromewebstore.google.com/detail/{STORE}"
    assert rows["extension_dir"] == "/x/chromium"
    by = {(b["browser"], p["folder"]): p for b in rows["browsers"] for p in b["profiles"]}
    assert by[("chrome", "Default")]["state"] == "on" and by[("chrome", "Default")]["name"] == "Personal"
    assert by[("chrome", "Profile 1")]["state"] == "installed" and by[("chrome", "Profile 1")]["ferb_identity"]
    assert by[("brave", "Default")]["state"] == "paired" and by[("brave", "Default")]["connected"]
    assert by[("edge", "Default")]["state"] == "not_installed"
    assert by[("edge", "Profile 1")]["state"] == "removed"
    assert {b["browser"]: b["manual"] for b in rows["browsers"]} == {"chrome": False, "brave": True, "edge": True,
                                                                     "vivaldi": True}
    assert {b["browser"]: b["label"] for b in rows["browsers"]}["chrome"] == "Google Chrome"
    assert browser_rows(states, "", Path("/x"), None, set())["store_url"] is None


def _point_daemon_at(daemon: RunningDaemon, world: dict[str, Any]) -> None:
    d = daemon.daemon
    d.config.set("browser", "store_extension_id", STORE)
    d.browser_data_dirs = {b: profiles.data_dir(b, world["support"]) for b in profiles.installed(world["support"])}


def test_the_daemon_lists_browsers_for_the_setup_window_without_writing(daemon: RunningDaemon,
                                                                       world: dict[str, Any]) -> None:
    _point_daemon_at(daemon, world)
    before = browser_files(world["support"])
    client = Client(daemon.sock)
    try:
        rows = client.call("setup.browsers")
    finally:
        client.close()
    assert browser_files(world["support"]) == before
    assert not (world["support"] / "Google" / "Chrome" / "External Extensions").exists()
    assert {b["browser"] for b in rows["browsers"]} == {"chrome", "brave", "edge", "vivaldi"}


def test_add_to_browser_opens_the_store_page_in_that_profile_only(daemon: RunningDaemon, world: dict[str, Any]) -> None:
    _point_daemon_at(daemon, world)
    launched: list[list[str]] = []
    daemon.daemon.browser_launch = launched.append
    page = f"https://chromewebstore.google.com/detail/{STORE}"
    client = Client(daemon.sock)
    try:
        assert client.call("setup.browser_open", {"browser": "chrome", "folder": "Profile 1"}) == {"opened": page}
        with pytest.raises(RpcError):
            client.call("setup.browser_open", {"browser": "chrome", "folder": "Profile 9"})   # not discovered
    finally:
        client.close()
    assert launched == [[profiles.BROWSERS["chrome"].binary, "--profile-directory=Profile 1", page]]
```

- [ ] **Step 2: Run them to verify they fail**

Run: `nice -n 10 uv run pytest -q tests/test_extension_autoinstall.py -k "read_states or setup_rows or setup_window or add_to_browser"`
Expected: FAIL — `AttributeError: module 'ferb.act.browser.autoinstall' has no attribute 'read_states'`, `ImportError:
cannot import name 'browser_rows'`.

- [ ] **Step 3: Implement `read_states`** in `ferb/act/browser/autoinstall.py`, right after `prepare`:

```python
def read_states(support: Path, store_id: str, ids: tuple[str, ...], paired: set[tuple[str, str]] | None,
                data_dirs: dict[str, Path] | None = None) -> list[BrowserState]:
    """Each installed browser's state as `prepare` reports it, read only: nothing is written (L34 C: the setup window
    polls it). `request` is the install request file when one is already there. `data_dirs` (browser -> user-data
    folder) replaces discovery under `support` (the daemon's own mapping; tests)."""
    dirs = data_dirs if data_dirs is not None else {b: profiles.data_dir(b, support) for b in profiles.installed(support)}
    out = []
    for browser, data in dirs.items():
        request = data / "External Extensions" / f"{store_id}.json" if valid_id(store_id) else None
        out.append(BrowserState(browser, request if request is not None and request.is_file() else None,
                                tuple(profile_status(p, ids, data, paired, store_id)
                                      for p in profiles.discover(browser, data))))
    return out
```

- [ ] **Step 4: Implement the rows** — append to `ferb/device/extensions.py` (add `from typing import Any` to the imports):

```python
# ---------- L34 C: the setup window's Browsers step ----------

def store_page(store_id: str) -> str:
    """FERB's Chrome Web Store page (Add to <browser>), or "" when no valid store id is set."""
    from ferb.act.browser.autoinstall import valid_id
    return f"https://chromewebstore.google.com/detail/{store_id}" if valid_id(store_id) else ""


def profile_state(p: Any) -> str:
    """A profile as the setup window shows it: not_installed, removed (the owner removed the store copy, so the browser
    won't add it again), installed (waiting to be turned on), on, or paired."""
    if not p.extension_id:
        return "removed" if p.removed else "not_installed"
    if not p.enabled:
        return "installed"
    return "paired" if p.paired else "on"


def browser_rows(states: list[Any], store_id: str, extension_dir: Path, ferb_identity: tuple[str, str] | None,
                 connected: set[tuple[str, str]]) -> dict[str, Any]:
    """`setup.browsers`'s reply: per browser whether FERB asked it to install the extension and whether Load unpacked is
    the way there (as `autoinstall.describe` says it), and per profile its state."""
    from ferb.act.browser import autoinstall, profiles
    valid = autoinstall.valid_id(store_id)
    return {"store_url": store_page(store_id) or None, "extension_dir": str(extension_dir),
            "browsers": [{"browser": s.browser, "label": profiles.BROWSERS[s.browser].label,
                          "requested": s.request is not None,
                          "manual": not valid or s.request is None or s.browser in autoinstall.UNCONFIRMED,
                          "unconfirmed": autoinstall.UNCONFIRMED.get(s.browser, ""),
                          "profiles": [{"folder": p.folder, "name": p.name, "state": profile_state(p),
                                        "ferb_identity": (p.browser, p.folder) == ferb_identity,
                                        "connected": (p.browser, p.folder) in connected} for p in s.profiles]}
                         for s in states]}
```

- [ ] **Step 5: Implement the RPCs**

`ferb/daemon/auth.py`: add `"setup.browsers", "setup.browser_open",` to the `Role.CLI` set.

`ferb/daemon/main.py` registration:

```python
        self.server.unary['setup.browsers'] = self._setup_browsers
        self.server.unary['setup.browser_open'] = self._setup_browser_open
```

Methods after `_browsers_unpair`:

```python
    async def _setup_browsers(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C: each browser and profile as the setup window shows it, read only (`autoinstall.read_states`): FERB's
        extension not added, removed, added, on or paired; whether FERB asked the browser to install it; where Load
        unpacked is the way. Nothing is written and no browser is opened."""
        from ferb.act.browser import autoinstall
        from ferb.act.browser.profiles import SUPPORT
        from ferb.device.extensions import browser_rows, extension_path
        store = autoinstall.store_extension_id(self.config)
        ids = autoinstall.accepted_extension_ids(self.config)
        paired = {(b.browser, b.folder) for b in self.pairings.all()} if self.pairings is not None else None
        states = await asyncio.to_thread(autoinstall.read_states, SUPPORT, store, ids, paired, self.browser_data_dirs)
        connected = {key for key, br in self.hub.bridges.items() if br.conn} if self.hub is not None else set()
        return browser_rows(states, store, extension_path(self.home), self._identity_profile(), connected)

    async def _setup_browser_open(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C: the owner's Add to <browser> on one profile: FERB's Web Store page opens in that profile, in front (the
        owner does the clicks; FERB never adds it for them, L25), or chrome://extensions when no store id is set (Load
        unpacked). Only a profile discovery returned; journaled."""
        from ferb.act.browser import autoinstall
        from ferb.device.extensions import store_page
        browser, folder = str(params.get("browser", "")), str(params.get("folder", ""))
        url = store_page(autoinstall.store_extension_id(self.config)) or "chrome://extensions"
        known = self._profiles()
        kwargs: dict[str, Any] = {"launch": self.browser_launch} if self.browser_launch else {}
        try:
            await asyncio.to_thread(browser_profiles.open_known_profile, browser, folder, url, known,
                                    background=False, **kwargs)
        except ValueError as e:
            raise FerbError(str(e)) from None
        self._journal().append("setup.browser_opened", {"browser": browser, "folder": folder, "url": url})
        return {"opened": url}
```

- [ ] **Step 6: Document** — add to the `docs/RPC.md` L34 table:

```markdown
| `setup.browsers` | `{}` | `{store_url, extension_dir, browsers: [{browser, label, requested, manual, unconfirmed, profiles: [{folder, name, state, ferb_identity, connected}]}]}`; `state`: `not_installed`, `removed`, `installed` (waiting to be turned on), `on` or `paired`. Read only: nothing written, no browser opened |
| `setup.browser_open` | `{browser, folder}` | Opens FERB's Chrome Web Store page (or `chrome://extensions` with no store id) in that discovered profile, in front, on the owner's click: `{opened: url}`; an unknown profile is refused; journaled `setup.browser_opened` |
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `nice -n 10 uv run pytest -q tests/test_extension_autoinstall.py tests/test_auth.py`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add ferb/act/browser/autoinstall.py ferb/device/extensions.py ferb/daemon/main.py ferb/daemon/auth.py docs/RPC.md \
  tests/test_extension_autoinstall.py
git commit -m "feat(L34): setup.browsers (read-only profile states) and setup.browser_open for Add to Chrome (L25)"
```

---

### Task 5: `ferb setup --service`: the background service with no questions

**Files:**
- Modify: `ferb/cli/setup.py` (new `service_setup`, `_foreign_daemon`; `setup()` uses `_foreign_daemon`)
- Modify: `ferb/cli/__init__.py` (`--service` flag and dispatch)
- Test: `tests/test_setup_flow.py`

**Interfaces:**
- Consumes: `_install`, `wait_for_daemon`, `extension_setup`, `_voice_helper`, `scan`, `ProfileStore`, `device_id`.
- Produces: `ferb.cli.setup.service_setup(env: Env, update: bool = False) -> int`; command
  `ferb setup --service [--update]` (exit 0 when the daemon answers; `InstallError` → `ferb: <reason>`, exit 1).

- [ ] **Step 1: Write the failing tests** (append to `tests/test_setup_flow.py`, after the `app_world` tests)

```python
# ---------- FERB.app's setup window (L34 C): `ferb setup --service` ----------

def test_the_setup_window_installs_the_service_without_asking_anything(app_world: dict[str, Any]) -> None:
    env, out = make_env(app_world, [], daemon_up=False)        # no answers: any question would fail the test
    assert S.service_setup(env) == 0, out.getvalue()
    text = out.getvalue()
    assert "FERB's background service is running." in text
    assert app_world["runner"].loaded
    assert app_world["panes"] == []                             # permissions are the window's own step
    calls = [m for m, _ in app_world["daemon"].calls]
    assert "setup.permission_request" not in calls and "setup.persona" not in calls and "doctor.check" not in calls
    assert (app_world["home"] / "device").is_dir()             # the app scan behind Automation's list
    assert Config(app_world["home"]).get("hearing", "requirement") == PINNED   # FERB Voice pinned for hearing


def test_the_setup_window_updates_the_service_quietly(app_world: dict[str, Any]) -> None:
    app_world["inst"].install()                                 # installed from this app before
    env, out = make_env(app_world, [])
    assert S.service_setup(env, update=True) == 0, out.getvalue()
    assert "Updating FERB's background service" in out.getvalue()
    assert [m for m, _ in app_world["daemon"].calls if m != "ping"] == []


def test_the_setup_window_refuses_a_disk_image_copy_with_its_fix(app_world: dict[str, Any], tmp_path: Path) -> None:
    from ferb.device.install import InstallError
    app_world["inst"] = Installer(app_world["home"], label="com.ferb.test", run=app_world["runner"],
                                  launch_agents=tmp_path / "LA", bundle=app_world["app"], cli_dir=tmp_path / "bin",
                                  read_only=lambda path: True)
    env, out = make_env(app_world, [], daemon_up=False)
    with pytest.raises(InstallError, match="drag FERB.app to Applications"):
        S.service_setup(env)
    assert not app_world["runner"].loaded


def test_the_setup_window_refuses_while_a_foreground_dev_daemon_holds_the_socket(world: dict[str, Any]) -> None:
    world["daemon"].pid = 999
    env, out = make_env(world, [])
    assert S.service_setup(env) == 1
    assert "outside launchd" in out.getvalue()


def test_ferb_setup_service_runs_the_quiet_step(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    from ferb.cli import main as cli_main
    ran: list[bool] = []
    monkeypatch.setenv("FERB_HOME", str(tmp_path / "home"))
    monkeypatch.setattr(S, "service_setup", lambda env, update=False: ran.append(update) or 0)
    assert cli_main(["setup", "--service", "--update"]) == 0
    assert cli_main(["setup", "--service"]) == 0
    assert ran == [True, False]
```

- [ ] **Step 2: Run them to verify they fail**

Run: `nice -n 10 uv run pytest -q tests/test_setup_flow.py -k "setup_window or setup_service"`
Expected: FAIL — `AttributeError: module 'ferb.cli.setup' has no attribute 'service_setup'` and argparse's
`unrecognized arguments: --service`.

- [ ] **Step 3: Implement** in `ferb/cli/setup.py`

Add above `setup()`:

```python
def _foreign_daemon(env: Env) -> str | None:
    """A FERB daemon answering outside launchd (`ferb daemon run` in a terminal) holds the socket, so setup can't start
    the installed one: says so, else None."""
    try:
        c = env.connect()
        pid = c.call("ping")["pid"]
        c.close()
    except NotRunning:
        return None
    if pid != env.inst.agent_pid():
        return (f"A FERB daemon (pid {pid}) is already running outside launchd — probably `ferb daemon run` "
                "in a terminal. Stop it (Ctrl-C) and run `ferb setup` again.")
    return None


def service_setup(env: Env, update: bool = False) -> int:
    """FERB.app's setup window (L34 C): install (or, with `update`, update) the background service from this app and
    wait until it answers, with no questions. Permissions, the AI service, browsers and the profile are the window's own
    steps, through the daemon's setup RPCs. An InstallError (a disk-image or translocated copy, a damaged app) reaches
    the caller with its fix."""
    inst = env.inst
    if not update and (refusal := _foreign_daemon(env)):
        env.say(refusal)
        return 1
    env.say("Updating FERB's background service…" if update else "Setting up FERB's background service…")
    env.say(env.tokenizer(inst.home))             # before the daemon starts: it counts tokens with it
    _install(env, update=update)
    wait_for_daemon(env).close()
    if not update:                                # the app list behind the Permissions step's Automation rows
        ProfileStore(inst.home).save(scan(device_id(Config(inst.home))), "setup scan")
    if inst.bundle is not None or (inst.home / "FERB.app").exists():
        _voice_helper(env)                        # pins FERB Voice for hearing and push-to-talk; never a prompt
    extension_setup(env)
    env.say("FERB's background service is running.")
    return 0
```

In `setup()`, replace the `try: … c.call("ping") … except NotRunning: pass` block with:

```python
    if refusal := _foreign_daemon(env):           # a foreground dev daemon would hold the socket
        env.say(refusal)
        return 1
```

In `ferb/cli/__init__.py`, after the `--voice` argument add:

```python
    sp.add_argument("--service", action="store_true",
                    help="FERB.app's setup window: install (with --update, update) the background service with no "
                         "questions and wait until it answers")
```

and in the setup dispatch, before `if args.cmd == "setup" and args.key:`:

```python
            if args.cmd == "setup" and args.service:
                return setup_cmd.service_setup(env, update=args.update)
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `nice -n 10 uv run pytest -q tests/test_setup_flow.py tests/test_cli.py tests/test_extension_autoinstall.py`
Expected: PASS (including `test_refuses_while_a_foreground_dev_daemon_holds_the_socket`).

- [ ] **Step 5: Commit**

```bash
git add ferb/cli/setup.py ferb/cli/__init__.py tests/test_setup_flow.py
git commit -m "feat(L34): ferb setup --service installs or updates the background service with no questions"
```

---

### Task 6: NotchCore: setup facts, parsers and the launch plan

**Files:**
- Create: `ui/mac/FERBNotch/Sources/NotchCore/SetupModels.swift`
- Modify: `ui/mac/FERBNotch/Sources/NotchCore/AppSetup.swift`
- Create: `ui/mac/FERBNotch/Tests/NotchCoreTests/SetupModelsTests.swift`
- Modify: `ui/mac/FERBNotch/Tests/NotchCoreTests/AppSetupTests.swift`

**Interfaces:**
- Produces (public, NotchCore): `SetupStep`, `GrantStatus` (`init(daemon:)`, `both(_:_:)`), `PermissionRow`
  (`init(id:status:process:required:pane:requestable:restartToCheck:blocked:)`, `init?(daemon:)`), `AutomationApp`,
  `PermissionSnapshot` (`init?(reply:)`; `rows`, `automation`, `automationPane`, `grantTarget`, `appPanes`,
  `setupDone`), `ProfileSetupState`, `BrowserProfileRow`, `BrowserRow`, `BrowserSnapshot` (`init?(reply:)`),
  `RuntimeRow`, `AISnapshot` (`init?(reply:)`, `current`, `usable(_:)`, `label(_:)`), `APIProvider`, `APIKeyEntry`
  (`account(_:)`, `isStorable(_:)`, `command(provider:key:)`), `PersonaState` (`init(reply:)`), `PermissionCopy`
  (`label(_:)`, `reason(_:)`, `status(_:)`), `LineSplitter`, `LaunchctlPlan.restart(uid:plist:plistExists:)`,
  `SetupLaunch`, `AppSetupPlan.serviceArguments(for:)`, `AppSetupPlan.atLaunch(_:setupDone:)`.
- Removes: `AppSetupPlan.arguments(for:fromMenu:)` (the console is gone).

- [ ] **Step 1: Write the failing tests** (`ui/mac/FERBNotch/Tests/NotchCoreTests/SetupModelsTests.swift`)

```swift
import XCTest
@testable import NotchCore

/// FERB.app's setup window (L34 C): the daemon's replies as the window reads them, and what it may run.
final class SetupModelsTests: XCTestCase {
    private let pane = "x-apple.systempreferences:com.apple.preference.security?"

    func testStepsRunInTheSpecsOrder() {
        XCTAssertEqual(SetupStep.allCases, [.welcome, .ai, .permissions, .browsers, .persona, .done])
        XCTAssertEqual(SetupStep.welcome.next, .ai)
        XCTAssertNil(SetupStep.done.next)
    }

    func testPermissionReplyKeepsWhoseGrantPaneAndBlock() {
        let reply: [String: Any] = [
            "rows": [
                ["id": "accessibility", "status": "denied", "process": "daemon", "required": true, "requestable": true,
                 "restart_to_check": false, "blocked": "", "pane": pane + "Privacy_Accessibility"],
                ["id": "full_disk_access", "status": "denied", "process": "daemon", "required": true,
                 "requestable": false, "restart_to_check": true,
                 "blocked": "FERB's shell sandbox and network proxy must be working first", "pane": pane + "Privacy_AllFiles"],
                ["status": "granted"]],
            "automation": [["name": "Notes", "bundle_id": "com.apple.Notes", "status": "not_determined"], ["name": "x"]],
            "automation_pane": pane + "Privacy_Automation",
            "grant_target": "/Applications/FERB.app/Contents/Helpers/FERB.app",
            "app_panes": ["notifications": "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=com.ferb.FERB"],
            "setup_done": true]
        let snapshot = PermissionSnapshot(reply: reply)!
        XCTAssertEqual(snapshot.rows.map(\.id), ["accessibility", "full_disk_access"])
        XCTAssertEqual(snapshot.rows[0].status, .denied)
        XCTAssertEqual(snapshot.rows[0].process, "daemon")
        XCTAssertTrue(snapshot.rows[0].requestable && snapshot.rows[0].required)
        XCTAssertEqual(snapshot.rows[0].pane?.absoluteString, pane + "Privacy_Accessibility")
        XCTAssertFalse(snapshot.rows[1].blocked.isEmpty)
        XCTAssertTrue(snapshot.rows[1].restartToCheck)
        XCTAssertEqual(snapshot.automation, [AutomationApp(name: "Notes", bundleID: "com.apple.Notes", status: .notDetermined)])
        XCTAssertEqual(snapshot.automationPane?.absoluteString, pane + "Privacy_Automation")
        XCTAssertEqual(snapshot.grantTarget, "/Applications/FERB.app/Contents/Helpers/FERB.app")
        XCTAssertNotNil(snapshot.appPanes["notifications"])
        XCTAssertTrue(snapshot.setupDone)
        XCTAssertNil(PermissionSnapshot(reply: ["ok": true]))
    }

    func testStatusesFERBDoesntKnowReadAsUnknown() {
        XCTAssertEqual(GrantStatus(daemon: "maybe"), .unknown)
        XCTAssertEqual(GrantStatus(daemon: nil), .unknown)
        XCTAssertEqual(GrantStatus(daemon: "target_not_running"), .targetNotRunning)
    }

    func testMicrophoneRowNeedsBothMicrophoneAndSpeech() {
        XCTAssertEqual(GrantStatus.both(.granted, .granted), .granted)
        XCTAssertEqual(GrantStatus.both(.granted, .denied), .denied)
        XCTAssertEqual(GrantStatus.both(.notDetermined, .granted), .notDetermined)
        XCTAssertEqual(GrantStatus.both(.unknown, .granted), .unknown)
    }

    func testBrowserReplyNamesEachProfilesState() {
        let snapshot = BrowserSnapshot(reply: [
            "store_url": "https://chromewebstore.google.com/detail/plbfkifcmgkgnlfgikdeaaglgncdhkma",
            "extension_dir": "/Users/o/.ferb/extensions/chromium",
            "browsers": [["browser": "chrome", "label": "Google Chrome", "requested": true, "manual": false,
                          "unconfirmed": "", "profiles": [
                            ["folder": "Default", "name": "Personal", "state": "on", "ferb_identity": false, "connected": false],
                            ["folder": "Profile 1", "name": "Work", "state": "not_installed"],
                            ["folder": "Profile 2", "name": "Odd", "state": "sideways"]]],
                         ["label": "no id"]]])!
        XCTAssertEqual(snapshot.browsers.map(\.browser), ["chrome"])
        XCTAssertEqual(snapshot.browsers[0].profiles.map(\.state), [.on, .notInstalled])
        XCTAssertTrue(snapshot.browsers[0].profiles[1].state.canAdd)
        XCTAssertFalse(snapshot.browsers[0].profiles[0].state.canAdd)
        XCTAssertEqual(snapshot.browsers[0].profiles[1].id, "chrome/Profile 1")
        XCTAssertNotNil(snapshot.storeURL)
        XCTAssertNil(BrowserSnapshot(reply: [:]))
    }

    func testAIReplyKnowsTheRuntimeEveryRoleUses() {
        let reply: [String: Any] = ["roles": ["orchestrator": "codex", "worker": "codex", "checker": "codex"],
                                    "runtimes": [["name": "codex", "usable": true, "fix": "", "model": "gpt-5.6-sol"],
                                                 ["name": "claude", "usable": false, "fix": "x", "model": ""]]]
        let snapshot = AISnapshot(reply: reply)!
        XCTAssertEqual(snapshot.current, "codex")
        XCTAssertTrue(snapshot.usable("codex"))
        XCTAssertFalse(snapshot.usable("claude"))
        XCTAssertFalse(snapshot.usable("google"))
        let mixed = AISnapshot(reply: ["roles": ["orchestrator": "codex", "worker": "claude", "checker": "claude"],
                                       "runtimes": [[String: Any]]()])!
        XCTAssertNil(mixed.current)
        XCTAssertEqual(AISnapshot.label("codex"), "ChatGPT (Codex)")
    }

    func testPersonaStatesFromTheDaemon() {
        XCTAssertEqual(PersonaState(reply: ["state": "scanning"]), .scanning)
        XCTAssertEqual(PersonaState(reply: ["state": "not_started"]), .loading)
        XCTAssertEqual(PersonaState(reply: ["state": "reviewed"]), .finished)
        XCTAssertEqual(PersonaState(reply: ["state": "unavailable"]), .finished)
        XCTAssertEqual(PersonaState(reply: ["state": "failed", "error": "OSError: x"]), .failed("OSError: x"))
        let pending = PersonaState(reply: ["state": "pending", "approval": [
            "approval_id": "p1", "reason": "Confirm or edit your app-use profile.", "proposal": "You code a lot.",
            "options": ["Save app-use profile", "Skip"]]])
        XCTAssertEqual(pending, .pending(approvalID: "p1", reason: "Confirm or edit your app-use profile.",
                                         proposal: "You code a lot.", canSave: true))
        let skipOnly = PersonaState(reply: ["state": "pending", "approval": ["approval_id": "p2", "options": ["Skip"]]])
        XCTAssertEqual(skipOnly, .pending(approvalID: "p2", reason: "", proposal: "", canSave: false))
        XCTAssertEqual(PersonaState(reply: ["state": "pending"]), .scanning)      // the question is being asked
    }

    func testAPIKeyLineCarriesTheKeyAsOneWordOrNothing() {
        XCTAssertEqual(APIKeyEntry.command(provider: .openai, key: "sk-proj-AbC_123"),
                       "add-generic-password -U -s ferb -a api/openai -w sk-proj-AbC_123\n")
        XCTAssertEqual(APIKeyEntry.account(.google), "api/google")
        for bad in ["", "two words", "quote\"d", "it's", "back\\slash", "tab\tkey", "ключ", "line\nbreak"] {
            XCTAssertNil(APIKeyEntry.command(provider: .anthropic, key: bad), bad)
        }
    }

    func testLinesArriveWholeAndTheLastOneOnFinish() {
        var lines = LineSplitter()
        XCTAssertEqual(lines.append("Setting up FERB's back"), [])
        XCTAssertEqual(lines.append("ground service…\n\n  Done\nferb: no"), ["Setting up FERB's background service…", "Done"])
        XCTAssertEqual(lines.finish(), "ferb: no")
        XCTAssertNil(lines.finish())
    }

    func testRestartKickstartsThenLoadsAnInstalledAgent() {
        let plist = "/Users/o/Library/LaunchAgents/com.ferb.daemon.plist"
        XCTAssertEqual(LaunchctlPlan.restart(uid: 501, plist: plist, plistExists: true),
                       [["kickstart", "-k", "gui/501/com.ferb.daemon"], ["bootstrap", "gui/501", plist]])
        XCTAssertEqual(LaunchctlPlan.restart(uid: 501, plist: plist, plistExists: false),
                       [["kickstart", "-k", "gui/501/com.ferb.daemon"]])
    }

    func testEveryRowHasWordsForTheOwner() {
        for id in ["accessibility", "screen_recording", "full_disk_access", "input_monitoring", "clipboard",
                   "notifications", "microphone", "hearing"] {
            XCTAssertNotEqual(PermissionCopy.label(id), id)
            XCTAssertFalse(PermissionCopy.reason(id).isEmpty)
        }
        XCTAssertEqual(PermissionCopy.status(.granted), "On")
    }
}
```

In `AppSetupTests.swift`, replace the uses of `AppSetupPlan.arguments(for:…)` and the menu test:

```swift
    func testNoInstalledAgentRunsSetup() {
        XCTAssertEqual(AppSetupPlan.decide(agent: nil, agentExecutable: agentExecutable, build: "202610140201"), .install)
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: .install), ["setup", "--service"])
    }

    func testAgentFromThisAppAndBuildNeedsNothing() {
        let installed = agent(agentExecutable, build: "202610140201")
        XCTAssertEqual(AppSetupPlan.decide(agent: installed, agentExecutable: agentExecutable, build: "202610140201"), .none)
        XCTAssertNil(AppSetupPlan.serviceArguments(for: .none))
    }

    func testReplacedAppUpdatesWithoutAsking() {
        let installed = agent(agentExecutable, build: "202610140201")
        XCTAssertEqual(AppSetupPlan.decide(agent: installed, agentExecutable: agentExecutable, build: "202610150900"), .update)
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: .update), ["setup", "--service", "--update"])
    }

    func testAnotherInstallationIsReplacedOnlyAfterTheOwnerSaysSo() {
        let source = agent("/Users/o/.ferb/venv/bin/python", build: nil)
        let action = AppSetupPlan.decide(agent: source, agentExecutable: agentExecutable, build: "202610140201")
        XCTAssertEqual(action, .confirmReplace(current: "/Users/o/.ferb/venv/bin/python"))
        XCTAssertEqual(AppSetupPlan.serviceArguments(for: action), ["setup", "--service"])
        XCTAssertTrue(AppSetupPlan.needsConfirmation(action))
    }

    func testLaunchOpensTheWindowOnlyWhenSomethingIsLeft() {
        XCTAssertEqual(AppSetupPlan.atLaunch(.install, setupDone: false), .window(.welcome))
        XCTAssertEqual(AppSetupPlan.atLaunch(.confirmReplace(current: "/x"), setupDone: true), .window(.welcome))
        XCTAssertEqual(AppSetupPlan.atLaunch(.update, setupDone: true), .quietUpdate)
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, setupDone: true), .nothing)
        XCTAssertEqual(AppSetupPlan.atLaunch(.none, setupDone: false), .window(.welcome))
        XCTAssertFalse(AppSetupPlan.needsConfirmation(.none))
    }
```

Delete `testMenuRunsFullSetupOnlyForThisAppAndAsksBeforeReplacingAnother` (its coverage is the two tests above plus
`SetupModelTests.testAnInstalledServiceIsNeverReinstalledFromWelcome`). Keep the other tests unchanged.

- [ ] **Step 2: Run them to verify they fail**

Run: `cd ui/mac/FERBNotch && swift test --filter "SetupModelsTests|AppSetupTests"`
Expected: FAIL to compile — `cannot find 'PermissionSnapshot' in scope`, `type 'AppSetupPlan' has no member
'serviceArguments'`.

- [ ] **Step 3: Implement `SetupModels.swift`**

```swift
import Foundation

/// FERB.app's setup window (L34 C): its steps, in the spec's order.
public enum SetupStep: Int, CaseIterable, Equatable {
    case welcome, ai, permissions, browsers, persona, done
    public var next: SetupStep? { SetupStep(rawValue: rawValue + 1) }
}

/// A macOS permission's state: the daemon's `permissions.Status` values, and FERB.app's own mapped onto them.
public enum GrantStatus: String, Equatable {
    case granted, denied, notDetermined = "not_determined", targetNotRunning = "target_not_running", unknown

    public init(daemon value: Any?) { self = GrantStatus(rawValue: value as? String ?? "") ?? .unknown }

    /// Two grants one row needs (dictation: Microphone and Speech Recognition).
    public static func both(_ a: GrantStatus, _ b: GrantStatus) -> GrantStatus {
        if a == .granted && b == .granted { return .granted }
        if a == .denied || b == .denied { return .denied }
        if a == .notDetermined || b == .notDetermined { return .notDetermined }
        return .unknown
    }
}

/// One Permissions row. `process` is whose grant it is: "daemon" (FERB's agent app, Contents/Helpers/FERB.app, which
/// runs the daemon), "voice" (FERB Voice) or "app" (FERB.app itself).
public struct PermissionRow: Equatable, Identifiable {
    public let id: String
    public let status: GrantStatus
    public let process: String
    public let required: Bool
    public let pane: URL?
    public let requestable: Bool
    public let restartToCheck: Bool
    public let blocked: String

    public init(id: String, status: GrantStatus, process: String, required: Bool, pane: URL?, requestable: Bool,
                restartToCheck: Bool = false, blocked: String = "") {
        self.id = id
        self.status = status
        self.process = process
        self.required = required
        self.pane = pane
        self.requestable = requestable
        self.restartToCheck = restartToCheck
        self.blocked = blocked
    }

    public init?(daemon row: [String: Any]) {
        guard let id = row["id"] as? String, !id.isEmpty else { return nil }
        self.init(id: id, status: GrantStatus(daemon: row["status"]), process: row["process"] as? String ?? "daemon",
                  required: row["required"] as? Bool ?? false, pane: (row["pane"] as? String).flatMap(URL.init(string:)),
                  requestable: row["requestable"] as? Bool ?? false,
                  restartToCheck: row["restart_to_check"] as? Bool ?? false, blocked: row["blocked"] as? String ?? "")
    }
}

public struct AutomationApp: Equatable, Identifiable {
    public var id: String { bundleID }
    public let name: String
    public let bundleID: String
    public let status: GrantStatus

    public init(name: String, bundleID: String, status: GrantStatus) {
        self.name = name
        self.bundleID = bundleID
        self.status = status
    }
}

/// `setup.permissions`'s reply.
public struct PermissionSnapshot: Equatable {
    public var rows: [PermissionRow]
    public let automation: [AutomationApp]
    public let automationPane: URL?
    public let grantTarget: String
    public let appPanes: [String: URL]
    public let setupDone: Bool

    public init?(reply: [String: Any]) {
        guard let rows = reply["rows"] as? [[String: Any]] else { return nil }
        self.rows = rows.compactMap(PermissionRow.init(daemon:))
        automation = (reply["automation"] as? [[String: Any]] ?? []).compactMap { row in
            guard let bundle = row["bundle_id"] as? String, !bundle.isEmpty else { return nil }
            return AutomationApp(name: row["name"] as? String ?? bundle, bundleID: bundle,
                                 status: GrantStatus(daemon: row["status"]))
        }
        automationPane = (reply["automation_pane"] as? String).flatMap(URL.init(string:))
        grantTarget = reply["grant_target"] as? String ?? ""
        appPanes = (reply["app_panes"] as? [String: String] ?? [:]).compactMapValues(URL.init(string:))
        setupDone = reply["setup_done"] as? Bool ?? false
    }
}

/// A browser profile as the Browsers step shows it (`extensions.profile_state`).
public enum ProfileSetupState: String, Equatable {
    case notInstalled = "not_installed", removed, installed, on, paired

    public var text: String {
        switch self {
        case .notInstalled: return "Not added"
        case .removed: return "Removed earlier"
        case .installed: return "Added; turn it on in the browser"
        case .on: return "On"
        case .paired: return "Paired"
        }
    }

    public var canAdd: Bool { self == .notInstalled || self == .removed }
}

public struct BrowserProfileRow: Equatable, Identifiable {
    public var id: String { browser + "/" + folder }
    public let browser: String
    public let folder: String
    public let name: String
    public let state: ProfileSetupState
    public let ferbIdentity: Bool
    public let connected: Bool
}

public struct BrowserRow: Equatable, Identifiable {
    public var id: String { browser }
    public let browser: String
    public let label: String
    public let requested: Bool
    public let manual: Bool
    public let unconfirmed: String
    public let profiles: [BrowserProfileRow]
}

/// `setup.browsers`'s reply.
public struct BrowserSnapshot: Equatable {
    public let storeURL: URL?
    public let extensionDir: String
    public let browsers: [BrowserRow]

    public init?(reply: [String: Any]) {
        guard let browsers = reply["browsers"] as? [[String: Any]] else { return nil }
        storeURL = (reply["store_url"] as? String).flatMap(URL.init(string:))
        extensionDir = reply["extension_dir"] as? String ?? ""
        self.browsers = browsers.compactMap { browser in
            guard let name = browser["browser"] as? String, !name.isEmpty else { return nil }
            let profiles = (browser["profiles"] as? [[String: Any]] ?? []).compactMap { p -> BrowserProfileRow? in
                guard let folder = p["folder"] as? String, !folder.isEmpty,
                      let state = ProfileSetupState(rawValue: p["state"] as? String ?? "") else { return nil }
                return BrowserProfileRow(browser: name, folder: folder, name: p["name"] as? String ?? folder,
                                         state: state, ferbIdentity: p["ferb_identity"] as? Bool ?? false,
                                         connected: p["connected"] as? Bool ?? false)
            }
            return BrowserRow(browser: name, label: browser["label"] as? String ?? name,
                              requested: browser["requested"] as? Bool ?? false,
                              manual: browser["manual"] as? Bool ?? false,
                              unconfirmed: browser["unconfirmed"] as? String ?? "", profiles: profiles)
        }
    }
}

public struct RuntimeRow: Equatable, Identifiable {
    public var id: String { name }
    public let name: String
    public let usable: Bool
    public let fix: String
    public let model: String
}

/// `runtimes.status`'s reply.
public struct AISnapshot: Equatable {
    public let roles: [String: String]
    public let runtimes: [RuntimeRow]

    public init?(reply: [String: Any]) {
        guard let roles = reply["roles"] as? [String: String], let rows = reply["runtimes"] as? [[String: Any]] else {
            return nil
        }
        self.roles = roles
        runtimes = rows.compactMap { row in
            guard let name = row["name"] as? String else { return nil }
            return RuntimeRow(name: name, usable: row["usable"] as? Bool ?? false, fix: row["fix"] as? String ?? "",
                              model: row["model"] as? String ?? "")
        }
    }

    /// The runtime every role uses, when they agree.
    public var current: String? {
        let used = Set(roles.values)
        return used.count == 1 ? used.first : nil
    }

    public func usable(_ name: String) -> Bool { runtimes.first { $0.name == name }?.usable ?? false }

    public static func label(_ runtime: String) -> String {
        ["codex": "ChatGPT (Codex)", "claude": "Claude", "anthropic": "your Anthropic API key",
         "openai": "your OpenAI API key", "google": "your Google API key"][runtime] ?? runtime
    }
}

public enum APIProvider: String, CaseIterable, Identifiable {
    case anthropic, openai, google
    public var id: String { rawValue }
    public var label: String {
        switch self {
        case .anthropic: return "Anthropic"
        case .openai: return "OpenAI"
        case .google: return "Google"
        }
    }
}

/// The Keychain item `ferb setup --key` writes (service "ferb", account "api/<provider>"), written by FERB.app on
/// `security -i`'s stdin (rule 14: never an argument).
public enum APIKeyEntry {
    public static func account(_ provider: APIProvider) -> String { "api/" + provider.rawValue }

    /// A key the Keychain line carries as one word: printable ASCII with no spaces, quotes or backslashes.
    public static func isStorable(_ key: String) -> Bool {
        !key.isEmpty && key.unicodeScalars.allSatisfy { scalar in
            scalar.value > 32 && scalar.value < 127 && scalar != "\"" && scalar != "'" && scalar != "\\"
        }
    }

    /// `security -i`'s line that stores it (`-U` replaces an earlier key for that provider), or nil.
    public static func command(provider: APIProvider, key: String) -> String? {
        isStorable(key) ? "add-generic-password -U -s ferb -a \(account(provider)) -w \(key)\n" : nil
    }
}

/// Getting to know you (`setup.persona`): the one profile question, as the daemon holds it.
public enum PersonaState: Equatable {
    case loading                 // not asked for yet: the window calls `setup.persona {}` to start the scan
    case scanning
    case pending(approvalID: String, reason: String, proposal: String, canSave: Bool)
    case finished                // reviewed, closed or unavailable: nothing to ask
    case failed(String)

    public init(reply: [String: Any]) {
        switch reply["state"] as? String {
        case "scanning": self = .scanning
        case "not_started": self = .loading
        case "pending":
            if let approval = reply["approval"] as? [String: Any], let id = approval["approval_id"] as? String {
                let options = approval["options"] as? [String] ?? []
                self = .pending(approvalID: id, reason: approval["reason"] as? String ?? "",
                                proposal: approval["proposal"] as? String ?? "",
                                canSave: options.contains("Save app-use profile"))
            } else {
                self = .scanning
            }
        case "failed": self = .failed(reply["error"] as? String ?? "The app-use scan failed.")
        default: self = .finished
        }
    }
}

/// The owner's words for each row.
public enum PermissionCopy {
    public static func label(_ id: String) -> String {
        ["accessibility": "Accessibility", "screen_recording": "Screen Recording", "full_disk_access": "Full Disk Access",
         "input_monitoring": "Input Monitoring", "clipboard": "Paste from Other Apps",
         "notifications": "Notifications", "microphone": "Microphone", "hearing": "Hearing app sound"][id] ?? id
    }

    public static func reason(_ id: String) -> String {
        ["accessibility": "FERB clicks and types in apps for you.",
         "screen_recording": "FERB sees the screen to check its work.",
         "full_disk_access": "FERB reads and tidies your files.",
         "input_monitoring": "FERB notices when you're typing, so it stays out of your way.",
         "clipboard": "FERB keeps what you copy on this Mac for 7 days. After macOS asks, set FERB to Allow.",
         "notifications": "FERB tells you when it needs you or finishes.",
         "microphone": "Dictate requests by voice (Microphone and Speech Recognition).",
         "hearing": "FERB turns an app's sound into text during a task, on this Mac only (FERB Voice)."][id] ?? ""
    }

    public static func status(_ status: GrantStatus) -> String {
        switch status {
        case .granted: return "On"
        case .denied: return "Off"
        case .notDetermined: return "Not asked yet"
        case .targetNotRunning: return "App not running"
        case .unknown: return "Unknown"
        }
    }
}

/// A command's output as whole lines (the setup window shows the latest one).
public struct LineSplitter {
    private var partial = ""

    public init() {}

    public mutating func append(_ text: String) -> [String] {
        partial += text
        var parts = partial.components(separatedBy: "\n")
        partial = parts.removeLast()
        return parts.map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
    }

    public mutating func finish() -> String? {
        let rest = partial.trimmingCharacters(in: .whitespaces)
        partial = ""
        return rest.isEmpty ? nil : rest
    }
}

/// Restarting FERB's background service (`com.ferb.daemon`): kickstart it when launchd has it; load it when it is
/// installed but not loaded.
public enum LaunchctlPlan {
    public static let label = "com.ferb.daemon"

    public static func restart(uid: uid_t, plist: String, plistExists: Bool) -> [[String]] {
        var commands = [["kickstart", "-k", "gui/\(uid)/\(label)"]]
        if plistExists { commands.append(["bootstrap", "gui/\(uid)", plist]) }
        return commands
    }
}
```

- [ ] **Step 4: Update `AppSetup.swift`**

Replace the doc comment above `AppSetupAction` with:

```swift
/// What the packaged FERB.app does at launch or from its menu (L27, L34 C). `ferb setup --service` does the work and
/// enforces where FERB may be installed from; this only reads the installed LaunchAgent
/// (~/Library/LaunchAgents/com.ferb.daemon.plist) and whether launchd has it running.
```

Add after the `AppSetupAction` enum:

```swift
/// What launch shows (L34 C).
public enum SetupLaunch: Equatable {
    case nothing                 // this app's build runs and setup is finished
    case window(SetupStep)       // the setup window at that step
    case quietUpdate             // `ferb setup --service --update`, no window unless it fails or a grant is missing
}
```

Inside `AppSetupPlan`, replace `arguments(for:fromMenu:)` with:

```swift
    /// The bundled command that installs or updates the background service with no questions; nil when this app's own
    /// build already runs it.
    public static func serviceArguments(for action: AppSetupAction) -> [String]? {
        switch action {
        case .none: return nil
        case .install, .confirmReplace: return ["setup", "--service"]
        case .update: return ["setup", "--service", "--update"]
        }
    }

    /// At launch: the setup window for a new install or another installation (after the owner's yes), a quiet update for
    /// a replaced build or a stopped service, and otherwise the window only while setup isn't finished.
    public static func atLaunch(_ action: AppSetupAction, setupDone: Bool) -> SetupLaunch {
        switch action {
        case .install, .confirmReplace: return .window(.welcome)
        case .update: return .quietUpdate
        case .none: return setupDone ? .nothing : .window(.welcome)
        }
    }
```

Change the `codexLoginArguments` comment to: `/// FERB's own Codex sign-in (C2): Choose your AI's Sign in with ChatGPT runs it.`

The old console controller still builds its arguments until Task 8 rewrites it. In `AppSetupController.swift`, replace
`AppSetupPlan.arguments(for: action, fromMenu: fromMenu)` with `Self.consoleArguments(for: action, fromMenu: fromMenu)`
and add this private helper to the class (Task 8 deletes it with the console):

```swift
    private static func consoleArguments(for action: AppSetupAction, fromMenu: Bool) -> [String]? {
        switch action {
        case .none: return fromMenu ? ["setup"] : nil
        case .install, .confirmReplace: return ["setup"]
        case .update: return ["setup", "--update"]
        }
    }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd ui/mac/FERBNotch && swift test --filter "SetupModelsTests|AppSetupTests"`
Expected: PASS, and the package builds.

- [ ] **Step 6: Commit**

```bash
git add ui/mac/FERBNotch/Sources/NotchCore/SetupModels.swift ui/mac/FERBNotch/Sources/NotchCore/AppSetup.swift \
  ui/mac/FERBNotch/Tests/NotchCoreTests/SetupModelsTests.swift ui/mac/FERBNotch/Tests/NotchCoreTests/AppSetupTests.swift \
  ui/mac/FERBNotch/Sources/FERBNotch/AppSetupController.swift
git commit -m "feat(L34): NotchCore setup facts: replies, persona, API key line, launch plan"
```

---

### Task 7: `SetupModel`: every step's reads and actions, with fakes

**Files:**
- Create: `ui/mac/FERBNotch/Sources/FERBNotch/SetupServices.swift` (protocols + real implementations)
- Create: `ui/mac/FERBNotch/Sources/FERBNotch/SetupModel.swift`
- Create: `ui/mac/FERBNotch/Tests/NotchCoreTests/Support/ScriptedRPC.swift`
- Create: `ui/mac/FERBNotch/Tests/NotchCoreTests/Support/SetupFakes.swift`
- Create: `ui/mac/FERBNotch/Tests/NotchCoreTests/SetupModelTests.swift`

**Interfaces:**
- Consumes: Task 6's NotchCore types; RPCs from Tasks 2–4; existing `automation.request`, `setup.persona`,
  `chat.approve`, `chat.answer`, `runtimes.check`; `DaemonRPCTransport.call(_:params:wait:)`.
- Produces (FERBNotch):
  - `@MainActor protocol SetupCommandRunning: AnyObject { var isRunning: Bool { get }; func run(_ arguments: [String], line: @escaping (String) -> Void, done: @escaping (Int32) -> Void) -> Bool; func cancel() }`
  - `@MainActor protocol AppPermissionsProviding: AnyObject { func notifications() async -> GrantStatus; func requestNotifications() async -> GrantStatus; func microphone() -> GrantStatus; func requestMicrophone() async -> GrantStatus; var microphoneDeniedPane: String { get } }`
  - `@MainActor protocol SystemOpening: AnyObject { func open(_ url: URL); func reveal(_ path: String) }`
  - `@MainActor protocol APIKeyStoring { func store(provider: APIProvider, key: String) async -> Bool }`
  - `@MainActor protocol DaemonServicing: AnyObject { func restart() async -> Bool }`
  - Real: `SetupCommand(launcher:)`, `SystemAppPermissions`, `SystemOpener`, `KeychainAPIKeyStore`, `DaemonService`,
    `enum AppBundle { static var isApp: Bool }`.
  - `@MainActor final class SetupModel: ObservableObject` with the members listed in "Interfaces for other phases"
    (the Claude sign-in's `signInWithClaude()`, `sendClaudeCode(_:)`, `cancelClaudeSignIn()` come in Task 10),
    `askForNotificationsOnce(defaults: UserDefaults) async -> Bool` (owner decision 6), plus `step`, `serviceAction`, `serviceRunning`, `serviceLine`, `serviceError`, `ai`, `aiMessage`, `signingIn`,
    `permissions`, `hearingRow`, `appRows`, `allRows`, `askedRestart`, `restarting`, `offersRestart`,
    `showsFullDiskHint`, `browsers`, `persona`, `personaMessage`, `message`, `primaryTitle`, `primaryDisabled`,
    `onFinish`, `go(_:)`, `enter(_:)`, `primary()`, `continueFromWelcome()`, `runService(completion:)`,
    `refreshCurrentStep()`, `cancelSignIn()`, `needsPermissions()`, `setupDone(attempts:pause:)`, `refreshPersona()`,
    `savePersona()`, `editPersona(_:)`, `skipPersona()`, `finish()`, `static let notAnswering: String`.
  - Test support Phase D reuses: `ScriptedRPC` (`replies`, `failures`, `events`, `calls`, `methods()`,
    `params(of:)`).

- [ ] **Step 1: Write the test support**

`Tests/NotchCoreTests/Support/ScriptedRPC.swift`:

```swift
import Foundation
import NotchCore

/// A daemon fake for the setup window and notification tests: canned replies per method, scripted failures, the events
/// a stream yields, and every call recorded with its params.
final class ScriptedRPC: DaemonRPCTransport, @unchecked Sendable {
    private let recorded = RecordedCalls()
    var calls: [(String, [String: Any])] { recorded.all }
    var replies: [String: [String: Any]] = [:]
    var failures: [String: Error] = [:]
    var events: [String: [[String: Any]]] = [:]

    func call(_ method: String, params: [String: Any]) async throws -> [String: Any] {
        recorded.append(method, params)
        if let failure = failures[method] { throw failure }
        return replies[method] ?? ["ok": true]
    }

    func stream(_ method: String, params: [String: Any]) -> AsyncThrowingStream<[String: Any], Error> {
        recorded.append(method, params)
        let items = events[method] ?? []
        return AsyncThrowingStream { continuation in
            for item in items { continuation.yield(item) }
            continuation.finish()
        }
    }

    func methods() -> [String] { calls.map(\.0) }
    func params(of method: String) -> [[String: Any]] { calls.filter { $0.0 == method }.map(\.1) }
}
```

`Tests/NotchCoreTests/Support/SetupFakes.swift`:

```swift
import Foundation
import XCTest
@testable import FERBNotch
import NotchCore

@MainActor
final class FakeSetupRunner: SetupCommandRunning {
    private(set) var runs: [[String]] = []
    private(set) var isRunning = false
    var startFails = false
    private var line: ((String) -> Void)?
    private var done: ((Int32) -> Void)?

    func run(_ arguments: [String], line: @escaping (String) -> Void, done: @escaping (Int32) -> Void) -> Bool {
        guard !startFails else { return false }
        runs.append(arguments)
        isRunning = true
        self.line = line
        self.done = done
        return true
    }

    func cancel() { finish(130) }
    func say(_ text: String) { line?(text) }

    func finish(_ status: Int32) {
        isRunning = false
        let finished = done
        done = nil
        line = nil
        finished?(status)
    }
}

@MainActor
final class FakeAppPermissions: AppPermissionsProviding {
    var notificationStatus: GrantStatus = .notDetermined
    var microphoneStatus: GrantStatus = .notDetermined
    var microphoneDeniedPane = "microphone"
    private(set) var requests: [String] = []

    func notifications() async -> GrantStatus { notificationStatus }
    func requestNotifications() async -> GrantStatus {
        requests.append("notifications")
        notificationStatus = .granted
        return .granted
    }
    func microphone() -> GrantStatus { microphoneStatus }
    func requestMicrophone() async -> GrantStatus {
        requests.append("microphone")
        microphoneStatus = .granted
        return .granted
    }
}

@MainActor
final class FakeOpener: SystemOpening {
    private(set) var opened: [URL] = []
    private(set) var revealed: [String] = []
    func open(_ url: URL) { opened.append(url) }
    func reveal(_ path: String) { revealed.append(path) }
}

@MainActor
final class FakeKeys: APIKeyStoring {
    private(set) var providers: [APIProvider] = []
    var accept = true
    func store(provider: APIProvider, key: String) async -> Bool {
        providers.append(provider)                  // the key itself is never kept, here or anywhere
        return accept && !key.isEmpty
    }
}

@MainActor
final class FakeService: DaemonServicing {
    private(set) var restarts = 0
    var result = true
    func restart() async -> Bool {
        restarts += 1
        return result
    }
}

/// One setup window's collaborators, all faked.
@MainActor
struct SetupRig {
    let rpc = ScriptedRPC()
    let runner = FakeSetupRunner()
    let app = FakeAppPermissions()
    let opener = FakeOpener()
    let keys = FakeKeys()
    let service = FakeService()

    func model(_ action: AppSetupAction = .none) -> SetupModel {
        let model = SetupModel(rpc: rpc, runner: runner, app: app, system: opener, keys: keys, service: service)
        model.serviceAction = action
        return model
    }
}

/// `setup.permissions` replies.
enum SetupReplies {
    static let pane = "x-apple.systempreferences:com.apple.preference.security?"
    static let agent = "/Applications/FERB.app/Contents/Helpers/FERB.app"

    static func permissions(accessibility: String = "denied", screen: String = "granted", fda: String = "denied",
                            fdaBlocked: String = "", setupDone: Bool = false, hearing: String? = nil) -> [String: Any] {
        func row(_ id: String, _ status: String, _ anchor: String, required: Bool, requestable: Bool,
                 restart: Bool = false, blocked: String = "", process: String = "daemon") -> [String: Any] {
            ["id": id, "status": status, "process": process, "required": required, "pane": pane + anchor,
             "requestable": requestable, "restart_to_check": restart, "blocked": blocked]
        }
        var rows = [row("accessibility", accessibility, "Privacy_Accessibility", required: true, requestable: true),
                    row("screen_recording", screen, "Privacy_ScreenCapture", required: true, requestable: true,
                        restart: true),
                    row("full_disk_access", fda, "Privacy_AllFiles", required: true, requestable: false, restart: true,
                        blocked: fdaBlocked),
                    row("input_monitoring", "granted", "Privacy_ListenEvent", required: false, requestable: true)]
        if let hearing {
            rows.append(row("hearing", hearing, "Privacy_SpeechRecognition", required: false, requestable: true,
                            process: "voice"))
        }
        return ["rows": rows, "automation": [["name": "Notes", "bundle_id": "com.apple.Notes", "status": "denied"],
                                             ["name": "Mail", "bundle_id": "com.apple.mail", "status": "not_determined"]],
                "automation_pane": pane + "Privacy_Automation", "grant_target": agent,
                "app_panes": ["notifications": "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=com.ferb.FERB",
                              "microphone": pane + "Privacy_Microphone",
                              "speech_recognition": pane + "Privacy_SpeechRecognition"],
                "setup_done": setupDone]
    }
}

/// Waits (at most 2 s) for something an unstructured Task does.
@MainActor
func eventually(_ condition: @escaping () -> Bool, file: StaticString = #filePath, line: UInt = #line) async {
    let end = ContinuousClock.now + .seconds(2)
    while !condition() {
        if ContinuousClock.now > end { XCTFail("condition never held", file: file, line: line); return }
        try? await Task.sleep(for: .milliseconds(10))
    }
}
```

- [ ] **Step 2: Write the failing tests** (`Tests/NotchCoreTests/SetupModelTests.swift`)

```swift
import XCTest
@testable import FERBNotch
@testable import NotchCore

/// FERB.app's setup window (L34 C): what each step reads and does, with the daemon, the bundled command, macOS and the
/// Keychain all faked.
@MainActor
final class SetupModelTests: XCTestCase {
    func testWelcomeInstallsTheServiceThenShowsChooseYourAI() async {
        let rig = SetupRig()
        let model = rig.model(.install)
        XCTAssertEqual(model.primaryTitle, "Set Up FERB")
        await model.primary()
        XCTAssertEqual(rig.runner.runs, [["setup", "--service"]])
        XCTAssertTrue(model.serviceRunning && model.primaryDisabled)
        rig.runner.say("Setting up FERB's background service…")
        XCTAssertEqual(model.serviceLine, "Setting up FERB's background service…")
        rig.runner.finish(0)
        XCTAssertFalse(model.serviceRunning)
        XCTAssertEqual(model.step, .ai)
        XCTAssertEqual(model.serviceAction, .none)              // going back to Welcome never reinstalls
    }

    func testAFailedServiceRunShowsItsLastLinesAndStaysOnWelcome() async {
        let rig = SetupRig()
        let model = rig.model(.install)
        await model.primary()
        rig.runner.say("Setting up FERB's background service…")
        rig.runner.say("ferb: FERB is running from its disk image: drag FERB.app to Applications, then open it from there")
        rig.runner.finish(1)
        XCTAssertEqual(model.step, .welcome)
        XCTAssertTrue(model.serviceError?.contains("drag FERB.app to Applications") == true)
        XCTAssertEqual(model.primaryTitle, "Try Again")
        await model.primary()
        XCTAssertEqual(rig.runner.runs.count, 2)
        XCTAssertNil(model.serviceError)
    }

    func testAnInstalledServiceIsNeverReinstalledFromWelcome() async {
        let rig = SetupRig()
        let model = rig.model(.none)
        XCTAssertEqual(model.primaryTitle, "Continue")
        await model.primary()
        XCTAssertTrue(rig.runner.runs.isEmpty)
        XCTAssertEqual(model.step, .ai)
    }

    func testGrantAsksTheDaemonForTheRequestThenOpensThatPane() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions()
        let model = rig.model()
        await model.refreshPermissions()
        await model.grant(model.allRows.first { $0.id == "accessibility" }!)
        XCTAssertEqual(rig.rpc.params(of: "setup.permission_request").first?["permission"] as? String, "accessibility")
        XCTAssertEqual(rig.opener.opened.map(\.absoluteString), [SetupReplies.pane + "Privacy_Accessibility"])
        XCTAssertTrue(rig.opener.revealed.isEmpty)
        XCTAssertFalse(model.offersRestart)                      // Accessibility applies without a restart
    }

    func testFullDiskAccessShowsTheAgentAppAndOffersARestartToCheck() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions()
        let model = rig.model()
        await model.refreshPermissions()
        await model.grant(model.allRows.first { $0.id == "full_disk_access" }!)
        XCTAssertTrue(rig.rpc.params(of: "setup.permission_request").isEmpty)   // no API: the owner adds FERB
        XCTAssertEqual(rig.opener.revealed, [SetupReplies.agent])
        XCTAssertEqual(rig.opener.opened.map(\.absoluteString), [SetupReplies.pane + "Privacy_AllFiles"])
        XCTAssertTrue(model.offersRestart && model.showsFullDiskHint)
        await model.restartService()
        XCTAssertEqual(rig.service.restarts, 1)
        XCTAssertFalse(model.offersRestart)
    }

    func testFullDiskAccessBeforeContainmentDoesNothing() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(
            fdaBlocked: "FERB's shell sandbox and network proxy must be working first")
        let model = rig.model()
        await model.refreshPermissions()
        let calls = rig.rpc.calls.count
        await model.grant(model.allRows.first { $0.id == "full_disk_access" }!)
        XCTAssertEqual(rig.rpc.calls.count, calls)
        XCTAssertTrue(rig.opener.opened.isEmpty && rig.opener.revealed.isEmpty)
    }

    func testFERBAppsOwnRowsAskMacOSWhenUndecidedAndOpenThePaneWhenOff() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions()
        rig.app.microphoneStatus = .denied
        rig.app.microphoneDeniedPane = "speech_recognition"
        let model = rig.model()
        await model.refreshPermissions()
        let notifications = model.allRows.first { $0.id == "notifications" }!
        XCTAssertEqual(notifications.process, "app")
        await model.grant(notifications)
        XCTAssertEqual(rig.app.requests, ["notifications"])
        XCTAssertTrue(rig.opener.opened.isEmpty)
        await model.grant(model.allRows.first { $0.id == "microphone" }!)
        XCTAssertEqual(rig.app.requests, ["notifications"])
        XCTAssertEqual(rig.opener.opened.last?.absoluteString, SetupReplies.pane + "Privacy_SpeechRecognition")
        XCTAssertEqual(model.allRows.first { $0.id == "notifications" }?.status, .granted)
    }

    func testHearingIsProbedOnEnteringTheStepAndAskedThroughFERBVoice() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(hearing: "not_determined")
        let model = rig.model()
        await model.enter(.permissions)
        XCTAssertEqual(rig.rpc.params(of: "setup.permissions").first?["hearing"] as? Bool, true)
        XCTAssertEqual(model.hearingRow?.process, "voice")
        XCTAssertEqual(model.allRows.filter { $0.id == "hearing" }.count, 1)
        await model.refreshPermissions()                         // a poll: no probe, the row stays
        XCTAssertNil(rig.rpc.params(of: "setup.permissions").last?["hearing"])
        XCTAssertNotNil(model.hearingRow)
        await model.grant(model.hearingRow!)
        XCTAssertEqual(rig.rpc.params(of: "setup.permission_request").last?["permission"] as? String, "hearing")
    }

    func testAutomationAllowAsksMacOSThroughTheDaemonOrOpensThePaneWhenOff() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions()
        let model = rig.model()
        await model.refreshPermissions()
        let apps = model.permissions!.automation
        await model.allowAutomation(apps.first { $0.bundleID == "com.apple.mail" }!)
        XCTAssertEqual(rig.rpc.params(of: "automation.request").first?["bundle_ids"] as? [String], ["com.apple.mail"])
        await model.allowAutomation(apps.first { $0.bundleID == "com.apple.Notes" }!)   // off: macOS won't ask again
        XCTAssertEqual(rig.rpc.params(of: "automation.request").count, 1)
        XCTAssertEqual(rig.opener.opened.last?.absoluteString, SetupReplies.pane + "Privacy_Automation")
    }

    func testAPIKeyGoesOnlyToTheKeychainThenThatServiceIsChosenAndChecked() async {
        let rig = SetupRig()
        rig.rpc.replies["runtimes.check"] = ["ok": true, "detail": "answered"]
        let model = rig.model()
        let secret = "sk-test-0123456789abcdef"
        await model.saveAPIKey(.openai, key: secret)
        XCTAssertEqual(rig.keys.providers, [.openai])
        XCTAssertEqual(rig.rpc.params(of: "runtimes.use").first?["runtime"] as? String, "openai")
        XCTAssertEqual(rig.rpc.params(of: "runtimes.check").first?["runtime"] as? String, "openai")
        for (_, params) in rig.rpc.calls { XCTAssertFalse(String(describing: params).contains(secret)) }
        let state = Mirror(reflecting: model).children.map { String(describing: $0.value) }.joined()
        XCTAssertFalse(state.contains(secret))
        XCTAssertEqual(model.aiMessage, "your OpenAI API key answered. FERB uses it now.")
    }

    func testAKeyTheKeychainLineCantCarryIsRefusedBeforeAnythingRuns() async {
        let rig = SetupRig()
        let model = rig.model()
        await model.saveAPIKey(.anthropic, key: "two words")
        XCTAssertTrue(rig.keys.providers.isEmpty)
        XCTAssertTrue(rig.rpc.calls.isEmpty)
    }

    func testSignInWithChatGPTRunsFERBsOwnCodexLoginThenChoosesCodex() async {
        let rig = SetupRig()
        let model = rig.model()
        model.signInWithChatGPT()
        XCTAssertEqual(rig.runner.runs, [["setup", "--codex-login"]])
        XCTAssertTrue(model.signingIn)
        rig.runner.finish(0)
        XCTAssertFalse(model.signingIn)
        await eventually { rig.rpc.methods().contains("runtimes.use") }
        XCTAssertEqual(rig.rpc.params(of: "runtimes.use").first?["runtime"] as? String, "codex")
        XCTAssertTrue(rig.rpc.params(of: "runtimes.check").isEmpty)    // signing in already proved it
    }

    func testUseClaudeChoosesItAndSaysWhetherItAnswered() async {
        let rig = SetupRig()
        rig.rpc.replies["runtimes.check"] = ["ok": false, "detail": "not logged in"]
        let model = rig.model()
        await model.useClaude()
        XCTAssertEqual(rig.rpc.params(of: "runtimes.use").first?["runtime"] as? String, "claude")
        XCTAssertEqual(model.aiMessage, "Claude didn't answer: not logged in")
    }

    func testPersonaSaveEditAndSkipAnswerTheOneQuestion() async {
        let pending: [String: Any] = ["state": "pending", "approval": [
            "approval_id": "p1", "reason": "Confirm or edit your app-use profile.", "proposal": "You code a lot.",
            "options": ["Save app-use profile", "Skip"]]]
        for action in ["save", "edit", "skip"] {
            let rig = SetupRig()
            rig.rpc.replies["setup.persona"] = pending
            let model = rig.model()
            await model.refreshPersona()
            switch action {
            case "save":
                await model.savePersona()
                let p = rig.rpc.params(of: "chat.approve").first
                XCTAssertEqual(p?["approval_id"] as? String, "p1")
                XCTAssertEqual(p?["granted"] as? Bool, true)
                XCTAssertEqual(p?["choice"] as? Int, 1)
            case "edit":
                await model.editPersona("I write Python and edit video")
                XCTAssertEqual(rig.rpc.params(of: "chat.answer").first?["text"] as? String, "I write Python and edit video")
            default:
                await model.skipPersona()
                XCTAssertEqual(rig.rpc.params(of: "chat.approve").first?["granted"] as? Bool, false)
            }
            XCTAssertEqual(model.persona, .finished, action)
        }
    }

    func testAPersonaAnswerFERBRefusesKeepsTheQuestionAndSaysWhy() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.persona"] = ["state": "pending", "approval": ["approval_id": "p1", "options": ["Skip"]]]
        rig.rpc.failures["chat.answer"] = DaemonRPCError.remote("There is no app-use evidence to confirm.")
        let model = rig.model()
        await model.refreshPersona()
        await model.editPersona("yes")
        XCTAssertEqual(model.personaMessage, "There is no app-use evidence to confirm.")
        XCTAssertEqual(model.persona, .pending(approvalID: "p1", reason: "", proposal: "", canSave: false))
    }

    func testAddToChromeAsksTheDaemonToOpenThatProfile() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.browsers"] = ["store_url": NSNull(), "extension_dir": "/x", "browsers": [
            ["browser": "chrome", "label": "Google Chrome", "profiles": [["folder": "Profile 1", "name": "Work",
                                                                          "state": "not_installed"]]]]]
        let model = rig.model()
        await model.refreshBrowsers()
        await model.addExtension(model.browsers!.browsers[0].profiles[0])
        let p = rig.rpc.params(of: "setup.browser_open").first
        XCTAssertEqual(p?["browser"] as? String, "chrome")
        XCTAssertEqual(p?["folder"] as? String, "Profile 1")
    }

    func testPollingThroughARestartKeepsTheStepAndSaysFERBIsAway() async {
        let rig = SetupRig()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions()
        let model = rig.model()
        model.step = .permissions
        await model.refreshPermissions()
        rig.rpc.failures["setup.permissions"] = DaemonRPCError.unavailable
        await model.refreshPermissions()
        XCTAssertEqual(model.step, .permissions)
        XCTAssertEqual(model.message, SetupModel.notAnswering)
        XCTAssertEqual(model.permissions?.rows.count, 4)          // the last rows stay on screen
        rig.rpc.failures["setup.permissions"] = nil
        await model.refreshPermissions()
        XCTAssertNil(model.message)
    }

    func testDoneRecordsSetupAndOpensTheNotch() async {
        let rig = SetupRig()
        let model = rig.model()
        var opened = false
        model.onFinish = { opened = true }
        model.step = .done
        XCTAssertEqual(model.primaryTitle, "Open FERB")
        await model.primary()
        XCTAssertTrue(rig.rpc.methods().contains("setup.finish"))
        XCTAssertTrue(opened)
    }

    func testLaunchKnowsSetupIsDoneFromTheDaemon() async {
        let rig = SetupRig()
        let model = rig.model()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(setupDone: true)
        let done = await model.setupDone(attempts: 1, pause: .zero)
        XCTAssertTrue(done)
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(setupDone: false)
        let notDone = await model.setupDone(attempts: 1, pause: .zero)
        XCTAssertFalse(notDone)
        rig.rpc.failures["setup.permissions"] = DaemonRPCError.unavailable
        let away = await model.setupDone(attempts: 2, pause: .zero)
        XCTAssertTrue(away)                                      // FERB isn't answering: never nag at launch
    }

    func testAQuietUpdateOpensPermissionsOnlyWhenARequiredGrantIsMissing() async {
        let rig = SetupRig()
        let model = rig.model()
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "granted")
        let fine = await model.needsPermissions()
        XCTAssertFalse(fine)
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "denied", fda: "granted")
        let missing = await model.needsPermissions()
        XCTAssertTrue(missing)
        rig.rpc.replies["setup.permissions"] = SetupReplies.permissions(accessibility: "granted", fda: "denied",
                                                                        fdaBlocked: "not yet")
        let blocked = await model.needsPermissions()
        XCTAssertFalse(blocked)                                   // a blocked row can't be granted yet
    }

    func testAnExistingInstallIsAskedForNotificationsOnceWhileMacOSHasntAsked() async {
        // owner decision 6: set up before this window existed, so its Permissions step never offered Notifications
        let rig = SetupRig()
        let model = rig.model()
        let defaults = UserDefaults(suiteName: "SetupModelTests-" + UUID().uuidString)!
        let asked = await model.askForNotificationsOnce(defaults: defaults)
        XCTAssertTrue(asked)
        XCTAssertEqual(rig.app.requests, ["notifications"])
        rig.app.notificationStatus = .notDetermined               // still undecided (dismissed): never asked again
        let again = await model.askForNotificationsOnce(defaults: defaults)
        XCTAssertFalse(again)
        XCTAssertEqual(rig.app.requests, ["notifications"])
        let decided = SetupRig()
        decided.app.notificationStatus = .denied                  // the owner already answered macOS: no question
        let fresh = UserDefaults(suiteName: "SetupModelTests-" + UUID().uuidString)!
        let none = await decided.model().askForNotificationsOnce(defaults: fresh)
        XCTAssertFalse(none)
        XCTAssertTrue(decided.app.requests.isEmpty)
    }
}
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cd ui/mac/FERBNotch && swift test --filter SetupModelTests`
Expected: FAIL to compile — `cannot find 'SetupModel' in scope`, `cannot find type 'SetupCommandRunning'`.

- [ ] **Step 4: Implement `SetupServices.swift`**

```swift
import AppKit
import AVFoundation
import Foundation
import NotchCore
@preconcurrency import Speech
import UserNotifications

/// Runs FERB.app's own `ferb` command for setup (L34 C): the background service (`setup --service [--update]`) and
/// FERB's Codex sign-in (`setup --codex-login`). One at a time; output arrives line by line.
@MainActor
protocol SetupCommandRunning: AnyObject {
    var isRunning: Bool { get }
    func run(_ arguments: [String], line: @escaping (String) -> Void, done: @escaping (Int32) -> Void) -> Bool
    func cancel()
}

/// FERB.app's own permissions: Notifications, and Microphone with Speech Recognition (dictation).
@MainActor
protocol AppPermissionsProviding: AnyObject {
    func notifications() async -> GrantStatus
    func requestNotifications() async -> GrantStatus
    func microphone() -> GrantStatus
    func requestMicrophone() async -> GrantStatus
    /// Which pane to open while the microphone row is off: "microphone" or "speech_recognition".
    var microphoneDeniedPane: String { get }
}

@MainActor
protocol SystemOpening: AnyObject {
    func open(_ url: URL)
    func reveal(_ path: String)
}

@MainActor
protocol APIKeyStoring {
    func store(provider: APIProvider, key: String) async -> Bool
}

@MainActor
protocol DaemonServicing: AnyObject {
    func restart() async -> Bool
}

enum AppBundle {
    /// Only an app bundle has a notification center (a `swift run` build has none).
    static var isApp: Bool { Bundle.main.bundleURL.pathExtension == "app" && Bundle.main.bundleIdentifier != nil }
}

/// The bundled `ferb` command, run from FERB.app's own Python. Quitting FERB stops it; `ferb setup` finishes
/// reinstalling the background service first (`_finish_install`). Setup never waits for an answer here (stdin is empty).
@MainActor
final class SetupCommand: SetupCommandRunning {
    private let launcher: URL
    private var process: Process?
    private var decoder = ConsoleDecoder()
    private var splitter = LineSplitter()
    private var outputEnded = false
    private var exitStatus: Int32?
    private var onLine: ((String) -> Void)?
    private var onDone: ((Int32) -> Void)?

    init(launcher: URL) { self.launcher = launcher }

    var isRunning: Bool { process != nil }

    func run(_ arguments: [String], line: @escaping (String) -> Void, done: @escaping (Int32) -> Void) -> Bool {
        guard process == nil else { return false }
        signal(SIGPIPE, SIG_IGN)
        decoder = ConsoleDecoder()
        splitter = LineSplitter()
        outputEnded = false
        exitStatus = nil
        onLine = line
        onDone = done
        let child = Process()
        child.executableURL = launcher
        child.arguments = arguments
        var environment = ProcessInfo.processInfo.environment
        environment["PYTHONUNBUFFERED"] = "1"
        child.environment = environment
        let pipe = Pipe()
        child.standardOutput = pipe
        child.standardError = pipe
        child.standardInput = FileHandle.nullDevice
        pipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            if data.isEmpty { handle.readabilityHandler = nil }
            DispatchQueue.main.async { self?.receive(data) }
        }
        child.terminationHandler = { [weak self] finished in
            let status = finished.terminationStatus
            DispatchQueue.main.async {
                self?.exitStatus = status
                self?.finishIfDone()
            }
        }
        do {
            try child.run()
        } catch {
            pipe.fileHandleForReading.readabilityHandler = nil
            onLine = nil
            onDone = nil
            return false
        }
        process = child
        return true
    }

    func cancel() {
        if let process, process.isRunning { process.terminate() }
    }

    private func receive(_ data: Data) {
        if data.isEmpty {
            outputEnded = true
            finishIfDone()
            return
        }
        for text in splitter.append(decoder.append(data)) { onLine?(text) }
    }

    private func finishIfDone() {
        guard outputEnded, let status = exitStatus, process != nil else { return }
        if let rest = splitter.finish() { onLine?(rest) }
        process = nil
        let done = onDone
        onLine = nil
        onDone = nil
        done?(status)
    }
}

/// FERB.app's own grants, read and requested by FERB.app itself.
@MainActor
final class SystemAppPermissions: AppPermissionsProviding {
    private var center: UNUserNotificationCenter? { AppBundle.isApp ? UNUserNotificationCenter.current() : nil }

    func notifications() async -> GrantStatus {
        guard let center else { return .unknown }
        return Self.grant(await center.notificationSettings().authorizationStatus)
    }

    func requestNotifications() async -> GrantStatus {
        guard let center else { return .unknown }
        _ = try? await center.requestAuthorization(options: [.alert, .sound])
        return await notifications()
    }

    func microphone() -> GrantStatus {
        GrantStatus.both(Self.grant(AVCaptureDevice.authorizationStatus(for: .audio)),
                         Self.grant(SFSpeechRecognizer.authorizationStatus()))
    }

    var microphoneDeniedPane: String {
        Self.grant(AVCaptureDevice.authorizationStatus(for: .audio)) == .denied ? "microphone" : "speech_recognition"
    }

    func requestMicrophone() async -> GrantStatus {
        if AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined {
            _ = await AVCaptureDevice.requestAccess(for: .audio)
        }
        if SFSpeechRecognizer.authorizationStatus() == .notDetermined {
            _ = await withCheckedContinuation { continuation in
                SFSpeechRecognizer.requestAuthorization { continuation.resume(returning: $0) }
            }
        }
        return microphone()
    }

    static func grant(_ status: UNAuthorizationStatus) -> GrantStatus {
        switch status {
        case .authorized, .provisional: return .granted
        case .denied: return .denied
        case .notDetermined: return .notDetermined
        @unknown default: return .unknown
        }
    }

    static func grant(_ status: AVAuthorizationStatus) -> GrantStatus {
        switch status {
        case .authorized: return .granted
        case .denied, .restricted: return .denied
        case .notDetermined: return .notDetermined
        @unknown default: return .unknown
        }
    }

    static func grant(_ status: SFSpeechRecognizerAuthorizationStatus) -> GrantStatus {
        switch status {
        case .authorized: return .granted
        case .denied, .restricted: return .denied
        case .notDetermined: return .notDetermined
        @unknown default: return .unknown
        }
    }
}

@MainActor
final class SystemOpener: SystemOpening {
    func open(_ url: URL) { NSWorkspace.shared.open(url) }
    func reveal(_ path: String) { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)]) }
}

/// An API key goes from the setup window straight into the Keychain (L34 C, rule 14): written on `security`'s stdin
/// (`security -i`), never as an argument other processes could read, never to the daemon, a log or FERB's settings. The
/// item is the one `ferb setup --key` writes, so the daemon's runtimes read it the same way. Only its presence is checked
/// afterwards, never its value.
@MainActor
struct KeychainAPIKeyStore: APIKeyStoring {
    func store(provider: APIProvider, key: String) async -> Bool {
        guard let command = APIKeyEntry.command(provider: provider, key: key) else { return false }
        let account = APIKeyEntry.account(provider)
        return await Task.detached {
            let writer = Process()
            writer.executableURL = URL(fileURLWithPath: "/usr/bin/security")
            writer.arguments = ["-i"]
            let input = Pipe()
            writer.standardInput = input
            writer.standardOutput = FileHandle.nullDevice
            writer.standardError = FileHandle.nullDevice
            do { try writer.run() } catch { return false }
            try? input.fileHandleForWriting.write(contentsOf: Data((command + "quit\n").utf8))
            try? input.fileHandleForWriting.close()
            writer.waitUntilExit()
            let check = Process()
            check.executableURL = URL(fileURLWithPath: "/usr/bin/security")
            check.arguments = ["find-generic-password", "-s", "ferb", "-a", account]   // no -w: presence only
            check.standardOutput = FileHandle.nullDevice
            check.standardError = FileHandle.nullDevice
            do { try check.run() } catch { return false }
            check.waitUntilExit()
            return check.terminationStatus == 0
        }.value
    }
}

/// FERB's background service through launchd (`LaunchctlPlan`): restarted after a Screen Recording or Full Disk Access
/// grant (macOS applies those to a new process), and by the menu's Restart (Phase D). Running tasks stop.
@MainActor
final class DaemonService: DaemonServicing {
    private let plist = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/LaunchAgents/com.ferb.daemon.plist")

    func restart() async -> Bool {
        let commands = LaunchctlPlan.restart(uid: getuid(), plist: plist.path,
                                             plistExists: FileManager.default.fileExists(atPath: plist.path))
        for command in commands where await Self.launchctl(command) == 0 { return true }
        return false
    }

    private static func launchctl(_ arguments: [String]) async -> Int32 {
        await Task.detached {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/bin/launchctl")
            process.arguments = arguments
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            do { try process.run() } catch { return -1 }
            process.waitUntilExit()
            return process.terminationStatus
        }.value
    }
}
```

- [ ] **Step 5: Implement `SetupModel.swift`**

```swift
import Foundation
import NotchCore

/// The setup window's state and actions (L34 C). Every fact comes from the daemon's setup RPCs or FERB.app's own
/// permission APIs; the window decides no grant and no choice itself.
@MainActor
final class SetupModel: ObservableObject {
    @Published var step: SetupStep = .welcome
    @Published var serviceAction: AppSetupAction = .none
    @Published private(set) var serviceRunning = false
    @Published private(set) var serviceLine = ""
    @Published private(set) var serviceError: String?
    @Published private(set) var ai: AISnapshot?
    @Published private(set) var aiMessage: String?
    @Published private(set) var signingIn = false
    @Published private(set) var permissions: PermissionSnapshot?
    @Published private(set) var hearingRow: PermissionRow?
    @Published private(set) var appRows: [PermissionRow] = []
    @Published private(set) var askedRestart: Set<String> = []
    @Published private(set) var restarting = false
    @Published private(set) var browsers: BrowserSnapshot?
    @Published private(set) var persona: PersonaState = .loading
    @Published private(set) var personaMessage: String?
    @Published var message: String?

    let rpc: DaemonRPCTransport
    let runner: SetupCommandRunning
    let app: AppPermissionsProviding
    let system: SystemOpening
    let keys: APIKeyStoring
    let service: DaemonServicing
    var onFinish: (() -> Void)?
    private var poll: Task<Void, Never>?

    static let notAnswering = "FERB isn't answering right now; this step updates when it's back."

    init(rpc: DaemonRPCTransport, runner: SetupCommandRunning, app: AppPermissionsProviding, system: SystemOpening,
         keys: APIKeyStoring, service: DaemonServicing) {
        self.rpc = rpc
        self.runner = runner
        self.app = app
        self.system = system
        self.keys = keys
        self.service = service
    }

    var allRows: [PermissionRow] { (permissions?.rows ?? []) + appRows + (hearingRow.map { [$0] } ?? []) }

    var offersRestart: Bool { allRows.contains { askedRestart.contains($0.id) && $0.status != .granted } }

    var showsFullDiskHint: Bool {
        askedRestart.contains("full_disk_access")
            && allRows.contains { $0.id == "full_disk_access" && $0.status != .granted }
    }

    var primaryTitle: String {
        switch step {
        case .welcome:
            if serviceError != nil { return "Try Again" }
            return AppSetupPlan.serviceArguments(for: serviceAction) == nil ? "Continue" : "Set Up FERB"
        case .done: return "Open FERB"
        default: return "Continue"
        }
    }

    var primaryDisabled: Bool { serviceRunning }

    // MARK: Steps

    func go(_ next: SetupStep) {
        step = next
        message = nil
        Task { await self.enter(next) }
    }

    /// What a step reads on entering it; polling keeps it fresh afterwards.
    func enter(_ step: SetupStep) async {
        switch step {
        case .permissions: await refreshPermissions(probeHearing: true)
        case .persona: await refreshPersona()
        default: await refreshCurrentStep()
        }
    }

    func primary() async {
        switch step {
        case .welcome: continueFromWelcome()
        case .done: await finish()
        default: if let next = step.next { go(next) }
        }
    }

    func continueFromWelcome() {
        guard AppSetupPlan.serviceArguments(for: serviceAction) != nil else { go(.ai); return }
        runService { [weak self] ok in if ok { self?.go(.ai) } }
    }

    /// `ferb setup --service [--update]`: the background service from this app, no questions. On failure its last lines
    /// (the reason and its fix) stay on Welcome.
    func runService(completion: @escaping (Bool) -> Void) {
        guard let arguments = AppSetupPlan.serviceArguments(for: serviceAction) else { completion(true); return }
        serviceError = nil
        serviceLine = "Starting…"
        serviceRunning = true
        var tail: [String] = []
        let started = runner.run(arguments, line: { [weak self] text in
            self?.serviceLine = text
            tail = Array((tail + [text]).suffix(6))
        }, done: { [weak self] status in
            guard let self else { return }
            self.serviceRunning = false
            if status == 0 {
                self.serviceAction = .none
                completion(true)
            } else {
                self.serviceError = tail.isEmpty ? "FERB's setup command stopped (exit status \(status))." :
                    tail.joined(separator: "\n")
                completion(false)
            }
        })
        if !started {
            serviceRunning = false
            serviceError = "FERB couldn't start its setup command."
            completion(false)
        }
    }

    // MARK: Polling

    func startPolling() {
        poll?.cancel()
        poll = Task { [weak self] in
            while !Task.isCancelled {
                await self?.refreshCurrentStep()
                do { try await Task.sleep(for: .seconds(2)) } catch { return }
            }
        }
    }

    func stopPolling() {
        poll?.cancel()
        poll = nil
    }

    func refreshCurrentStep() async {
        switch step {
        case .ai: await refreshAI()
        case .permissions: await refreshPermissions()
        case .browsers: await refreshBrowsers()
        case .persona: if persona == .loading || persona == .scanning { await refreshPersona() }
        case .welcome, .done: break
        }
    }

    private func clearNotAnswering() { if message == Self.notAnswering { message = nil } }

    // MARK: Choose your AI

    func refreshAI() async {
        do {
            ai = AISnapshot(reply: try await rpc.call("runtimes.status", params: [:]))
            clearNotAnswering()
        } catch { message = Self.notAnswering }
    }

    func signInWithChatGPT() {
        guard !runner.isRunning else { aiMessage = "Wait for the current step to finish."; return }
        signingIn = true
        aiMessage = "Finish signing in in the browser window that opened."
        let started = runner.run(AppSetupPlan.codexLoginArguments, line: { _ in }, done: { [weak self] status in
            guard let self else { return }
            self.signingIn = false
            if status == 0 { Task { await self.use("codex", check: false) } }
            else { self.aiMessage = "Not signed in. Try again when you're ready." }
        })
        if !started {
            signingIn = false
            aiMessage = "FERB couldn't start the sign-in."
        }
    }

    func cancelSignIn() { runner.cancel() }

    func useClaude() async { await use("claude", check: true) }

    func saveAPIKey(_ provider: APIProvider, key: String) async {
        guard APIKeyEntry.isStorable(key) else {
            aiMessage = "That doesn't look like an API key (it has spaces, quotes or other characters)."
            return
        }
        aiMessage = "Saving the key in your Keychain…"
        guard await keys.store(provider: provider, key: key) else {
            aiMessage = "The key wasn't saved in your Keychain."
            return
        }
        await use(provider.rawValue, check: true)
    }

    private func use(_ runtime: String, check: Bool) async {
        let name = AISnapshot.label(runtime)
        do {
            _ = try await rpc.call("runtimes.use", params: ["runtime": runtime])
            if check {
                aiMessage = "Checking that \(name) answers…"
                let reply = try await rpc.call("runtimes.check", params: ["runtime": runtime], wait: 60)
                aiMessage = reply["ok"] as? Bool == true ? "\(name) answered. FERB uses it now."
                    : "\(name) didn't answer: \(reply["detail"] as? String ?? "no answer")"
            } else {
                aiMessage = "FERB uses \(name) now."
            }
        } catch { aiMessage = error.localizedDescription }
        await refreshAI()
    }

    // MARK: Permissions

    func refreshPermissions(probeHearing: Bool = false) async {
        do {
            let reply = try await rpc.call("setup.permissions", params: probeHearing ? ["hearing": true] : [:],
                                           wait: probeHearing ? 30 : 10)
            if var snapshot = PermissionSnapshot(reply: reply) {
                if probeHearing { hearingRow = snapshot.rows.first { $0.id == "hearing" } }
                snapshot.rows.removeAll { $0.id == "hearing" }       // shown from hearingRow, probed on demand
                permissions = snapshot
            }
            clearNotAnswering()
        } catch { message = Self.notAnswering }
        let notifications = await app.notifications()
        let microphone = app.microphone()
        appRows = [
            PermissionRow(id: "notifications", status: notifications, process: "app", required: false,
                          pane: permissions?.appPanes["notifications"], requestable: notifications == .notDetermined),
            PermissionRow(id: "microphone", status: microphone, process: "app", required: false,
                          pane: permissions?.appPanes[app.microphoneDeniedPane], requestable: microphone == .notDetermined)]
    }

    /// The owner's Grant on one row: macOS's own request when there is one (from the daemon for its grants, from FERB.app
    /// for its own), then that pane; Full Disk Access also shows the agent app in Finder to drag into the list.
    func grant(_ row: PermissionRow) async {
        guard row.blocked.isEmpty else { return }
        switch row.id {
        case "notifications":
            if row.status == .notDetermined { _ = await app.requestNotifications() }
            else if let pane = row.pane { system.open(pane) }
        case "microphone":
            if row.status == .notDetermined { _ = await app.requestMicrophone() }
            else if let pane = row.pane { system.open(pane) }
        case "hearing":
            if row.status == .notDetermined {
                _ = try? await rpc.call("setup.permission_request", params: ["permission": "hearing"], wait: 120)
            } else if let pane = row.pane { system.open(pane) }
            await refreshPermissions(probeHearing: true)
            return
        default:
            if row.requestable {
                _ = try? await rpc.call("setup.permission_request", params: ["permission": row.id], wait: 120)
            }
            if row.id == "full_disk_access", let target = permissions?.grantTarget, !target.isEmpty {
                system.reveal(target)
            }
            if let pane = row.pane { system.open(pane) }
            if row.restartToCheck { askedRestart.insert(row.id) }
        }
        await refreshPermissions()
    }

    /// Automation for one running app: macOS asks once (through the daemon, so the grant is the daemon's); after a No it
    /// never asks again, so the pane opens instead.
    func allowAutomation(_ target: AutomationApp) async {
        if target.status == .denied {
            if let pane = permissions?.automationPane { system.open(pane) }
            return
        }
        _ = try? await rpc.call("automation.request", params: ["bundle_ids": [target.bundleID]], wait: 120)
        await refreshPermissions()
    }

    func restartService() async {
        restarting = true
        message = "Restarting FERB's background service…"
        let ok = await service.restart()
        restarting = false
        message = ok ? nil : "FERB's background service couldn't be restarted."
        if ok { askedRestart.removeAll() }
        await refreshPermissions()
    }

    /// After a quiet update: whether a required grant that can be granted now is missing.
    func needsPermissions() async -> Bool {
        await refreshPermissions()
        return permissions?.rows.contains { $0.required && $0.status != .granted && $0.blocked.isEmpty } ?? false
    }

    /// Whether setup was finished (by this window or `ferb setup`), asked of the daemon at launch.
    func setupDone(attempts: Int = 3, pause: Duration = .seconds(2)) async -> Bool {
        for attempt in 0..<attempts {
            if let reply = try? await rpc.call("setup.permissions", params: [:]),
               let snapshot = PermissionSnapshot(reply: reply) {
                return snapshot.setupDone
            }
            if attempt + 1 < attempts { try? await Task.sleep(for: pause) }
        }
        return true                 // FERB isn't answering: never nag at launch; its menu says it isn't running
    }

    /// Owner decision 6 (2026-10-25): a launch that shows no setup window (an install set up before this window existed)
    /// asks macOS once for FERB.app's Notifications while macOS hasn't asked; macOS shows its own Allow prompt, which
    /// takes no focus. Remembered in FERB.app's defaults (`FERBAskedNotifications`), so it is asked once. True when it
    /// asked.
    func askForNotificationsOnce(defaults: UserDefaults) async -> Bool {
        guard !defaults.bool(forKey: "FERBAskedNotifications"), await app.notifications() == .notDetermined else {
            return false
        }
        defaults.set(true, forKey: "FERBAskedNotifications")
        _ = await app.requestNotifications()
        return true
    }

    // MARK: Browsers

    func refreshBrowsers() async {
        do {
            browsers = BrowserSnapshot(reply: try await rpc.call("setup.browsers", params: [:]))
            clearNotAnswering()
        } catch { message = Self.notAnswering }
    }

    func addExtension(_ profile: BrowserProfileRow) async {
        do {
            _ = try await rpc.call("setup.browser_open", params: ["browser": profile.browser, "folder": profile.folder])
        } catch { message = error.localizedDescription }
    }

    // MARK: Getting to know you

    func refreshPersona() async {
        let params: [String: Any] = persona == .loading ? [:] : ["status": true]
        do {
            persona = PersonaState(reply: try await rpc.call("setup.persona", params: params))
            clearNotAnswering()
        } catch { message = Self.notAnswering }
    }

    func savePersona() async {
        guard case .pending(let id, _, _, true) = persona else { return }
        await answerPersona("chat.approve", ["approval_id": id, "granted": true, "choice": 1])
    }

    func editPersona(_ text: String) async {
        guard case .pending(let id, _, _, _) = persona,
              !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        await answerPersona("chat.answer", ["approval_id": id, "text": text])
    }

    func skipPersona() async {
        guard case .pending(let id, _, _, _) = persona else { return }
        await answerPersona("chat.approve", ["approval_id": id, "granted": false])
    }

    private func answerPersona(_ method: String, _ params: [String: Any]) async {
        do {
            _ = try await rpc.call(method, params: params)
            persona = .finished
            personaMessage = nil
        } catch { personaMessage = error.localizedDescription }
    }

    // MARK: Done

    func finish() async {
        _ = try? await rpc.call("setup.finish", params: [:])       // only records it; Done still opens the notch
        stopPolling()
        onFinish?()
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd ui/mac/FERBNotch && swift test --filter "SetupModelTests|SetupModelsTests"`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add ui/mac/FERBNotch/Sources/FERBNotch/SetupServices.swift ui/mac/FERBNotch/Sources/FERBNotch/SetupModel.swift \
  ui/mac/FERBNotch/Tests/NotchCoreTests/Support/ScriptedRPC.swift ui/mac/FERBNotch/Tests/NotchCoreTests/Support/SetupFakes.swift \
  ui/mac/FERBNotch/Tests/NotchCoreTests/SetupModelTests.swift
git commit -m "feat(L34): SetupModel: each setup step through the daemon's RPCs and FERB.app's own grants (rule 14)"
```

---

### Task 8: The setup window: SwiftUI views, the controller and the app's wiring

**Files:**
- Create: `ui/mac/FERBNotch/Sources/FERBNotch/SetupView.swift`
- Modify (rewrite): `ui/mac/FERBNotch/Sources/FERBNotch/AppSetupController.swift`
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/Application.swift`
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/PanelCoordinator.swift` (drop **Sign in to Codex…** and `onCodexLogin`)
- Test: `ui/mac/FERBNotch/Tests/NotchCoreTests/SetupModelTests.swift` (one view test)

**Interfaces:**
- Consumes: `SetupModel` and services (Task 7), `AppSetupPlan.atLaunch`/`decide` (Task 6).
- Produces: `SetupView(model:)`, `AIStepView(model:)`, `PermissionsStepView(model:)`, `BrowsersStepView(model:)` (Phase E
  embeds the last three); `AppSetupController.startAtLaunch()`, `.runSetupFromMenu()`, `.stop()`, `.onFinished`.

- [ ] **Step 1: Write the failing view test** (append inside `SetupModelTests`; add `import AppKit` and `import SwiftUI` at
  the top of the file)

```swift
    func testEveryStepDrawsWithoutTheDaemon() {
        let rig = SetupRig()
        let model = rig.model(.install)
        for step in SetupStep.allCases {
            model.step = step
            let view = NSHostingView(rootView: SetupView(model: model))
            view.frame = NSRect(x: 0, y: 0, width: 560, height: 500)
            view.layoutSubtreeIfNeeded()
            XCTAssertGreaterThan(view.fittingSize.height, 0, "\(step)")
        }
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd ui/mac/FERBNotch && swift test --filter SetupModelTests/testEveryStepDrawsWithoutTheDaemon`
Expected: FAIL to compile — `cannot find 'SetupView' in scope`.

- [ ] **Step 3: Implement `SetupView.swift`**

```swift
import SwiftUI
import NotchCore

/// FERB.app's setup window (L34 C): one title, two lines, one primary button, progress dots. Bare minimum on purpose;
/// the owner styles it later.
struct SetupView: View {
    @ObservedObject var model: SetupModel

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            switch model.step {
            case .welcome: WelcomeStep(model: model)
            case .ai: AIStepView(model: model)
            case .permissions: PermissionsStepView(model: model)
            case .browsers: BrowsersStepView(model: model)
            case .persona: PersonaStep(model: model)
            case .done: DoneStep()
            }
            Spacer(minLength: 0)
            if let message = model.message { Text(message).font(.callout).foregroundStyle(.secondary) }
            HStack {
                SetupDots(current: model.step)
                Spacer()
                Button(model.primaryTitle) { Task { await model.primary() } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(model.primaryDisabled)
            }
        }
        .padding(24)
        .frame(width: 560, height: 500)
    }
}

struct SetupDots: View {
    let current: SetupStep
    var body: some View {
        HStack(spacing: 6) {
            ForEach(SetupStep.allCases, id: \.rawValue) { step in
                Circle().fill(step == current ? Color.primary : Color.secondary.opacity(0.35)).frame(width: 7, height: 7)
            }
        }
    }
}

struct StepHeader: View {
    let title: String
    let lines: [String]
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.title2).bold()
            ForEach(lines, id: \.self) { Text($0).foregroundStyle(.secondary) }
        }
    }
}

struct WelcomeStep: View {
    @ObservedObject var model: SetupModel
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            StepHeader(title: "FERB works on your Mac in the background", lines: [
                "What FERB keeps (its memory, history and settings) stays on this Mac, in ~/.ferb.",
                "What FERB sends to the AI service you choose is redacted first: passwords, codes and keys never leave."])
            if model.serviceRunning {
                ProgressView()
                Text(model.serviceLine).font(.callout).lineLimit(2)
            }
            if let error = model.serviceError {
                Text("FERB couldn't set up its background service:").bold()
                ScrollView { Text(error).font(.system(.callout, design: .monospaced)).textSelection(.enabled) }
                    .frame(maxHeight: 140)
            }
        }
    }
}

struct AIStepView: View {
    @ObservedObject var model: SetupModel
    @State private var provider: APIProvider = .anthropic
    @State private var key = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            StepHeader(title: "Choose your AI", lines: ["FERB thinks with an AI service you already use.",
                                                        "Pick one; you can change it later."])
            if let current = model.ai?.current { Text("FERB uses: \(AISnapshot.label(current))").font(.callout) }
            HStack {
                Button(model.signingIn ? "Waiting for your browser…" : "Sign in with ChatGPT") { model.signInWithChatGPT() }
                    .disabled(model.signingIn)
                if model.signingIn { Button("Cancel") { model.cancelSignIn() } }
                if model.ai?.usable("codex") == true { Text("Signed in").foregroundStyle(.secondary) }
            }
            HStack {
                Button("Use Claude") { Task { await model.useClaude() } }
                Text("Your Claude subscription, signed in to Claude Code on this Mac").font(.caption)
                    .foregroundStyle(.secondary)
            }
            HStack {
                Picker("API key", selection: $provider) {
                    ForEach(APIProvider.allCases) { Text($0.label).tag($0) }
                }
                .frame(width: 190)
                SecureField("Paste your key", text: $key)
                Button("Save") {
                    let entered = key
                    key = ""                                   // the field never keeps it
                    Task { await model.saveAPIKey(provider, key: entered) }
                }
                .disabled(key.isEmpty)
            }
            if let line = model.aiMessage { Text(line).font(.callout) }
        }
    }
}

struct PermissionsStepView: View {
    @ObservedObject var model: SetupModel
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            StepHeader(title: "Let FERB work on your Mac", lines: ["macOS asks you to allow each of these once.",
                                                                   "Each row ticks itself when it's on."])
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(model.allRows) { row in PermissionRowView(row: row, model: model) }
                    if let apps = model.permissions?.automation, !apps.isEmpty {
                        Text("Automation: apps FERB may control").bold().padding(.top, 6)
                        ForEach(apps) { target in
                            HStack {
                                Text(target.name)
                                Spacer()
                                Text(PermissionCopy.status(target.status)).foregroundStyle(.secondary)
                                if target.status != .granted {
                                    Button("Allow") { Task { await model.allowAutomation(target) } }
                                }
                            }
                        }
                    }
                }
            }
            if model.offersRestart {
                Button(model.restarting ? "Restarting FERB…" : "Switched it on? Restart FERB to check") {
                    Task { await model.restartService() }
                }
                .disabled(model.restarting)
            }
            if model.showsFullDiskHint, let target = model.permissions?.grantTarget {
                Text("Full Disk Access: drag FERB from the Finder window into the list.\n\(target)")
                    .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
            }
        }
    }
}

struct PermissionRowView: View {
    let row: PermissionRow
    @ObservedObject var model: SetupModel
    var body: some View {
        HStack(alignment: .top) {
            Text(row.status == .granted ? "✓" : "○").frame(width: 16)
            VStack(alignment: .leading, spacing: 2) {
                Text(PermissionCopy.label(row.id) + (row.required ? "" : " (optional)"))
                Text(row.blocked.isEmpty ? PermissionCopy.reason(row.id) : row.blocked)
                    .font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            if row.status != .granted {
                Button("Grant") { Task { await model.grant(row) } }.disabled(!row.blocked.isEmpty)
            }
        }
    }
}

struct BrowsersStepView: View {
    @ObservedObject var model: SetupModel
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            StepHeader(title: "Browsers", lines: [
                "FERB works in your browsers through its extension.",
                "Add it where you want FERB's help; FERB pairs a profile by itself when you're in it."])
            if let snapshot = model.browsers {
                if snapshot.browsers.isEmpty {
                    Text("No Chrome-family browser has been opened on this Mac yet.").foregroundStyle(.secondary)
                }
                ScrollView {
                    VStack(alignment: .leading, spacing: 8) {
                        ForEach(snapshot.browsers) { browser in
                            Text(browser.label).bold()
                            ForEach(browser.profiles) { profile in
                                HStack {
                                    Text(profile.name + (profile.ferbIdentity ? " (FERB's own)" : ""))
                                    Spacer()
                                    Text(profile.state.text).foregroundStyle(.secondary)
                                    if profile.state.canAdd {
                                        Button("Add to \(browser.label)") { Task { await model.addExtension(profile) } }
                                    }
                                }
                            }
                            if browser.manual {
                                Text("Or load it unpacked from the browser's extensions page (Developer mode): \(snapshot.extensionDir)")
                                    .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                            }
                        }
                    }
                }
            } else {
                ProgressView()
            }
        }
    }
}

struct PersonaStep: View {
    @ObservedObject var model: SetupModel
    @State private var words = ""
    @State private var editing = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            StepHeader(title: "Getting to know you", lines: ["FERB looked at which apps you use, on this Mac only.",
                                                             "Save what it found, say it in your own words, or skip."])
            switch model.persona {
            case .loading, .scanning:
                ProgressView("Looking at your apps…")
            case .pending(_, let reason, let proposal, let canSave):
                if !reason.isEmpty { Text(reason) }
                if !proposal.isEmpty { Text(proposal).font(.callout).textSelection(.enabled) }
                if editing {
                    TextField("How you use this Mac, in your words", text: $words, axis: .vertical).lineLimit(3...6)
                    HStack {
                        Button("Save") {
                            let text = words
                            Task { await model.editPersona(text) }
                        }
                        .disabled(words.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                        Button("Cancel") { editing = false }
                    }
                } else {
                    HStack {
                        if canSave { Button("Save") { Task { await model.savePersona() } } }
                        Button("Edit") { editing = true }
                        Button("Skip") { Task { await model.skipPersona() } }
                    }
                }
            case .finished:
                Text("Done. FERB keeps this in its memory; you can change it later.").foregroundStyle(.secondary)
            case .failed(let why):
                Text("FERB couldn't look at your apps: \(why)").foregroundStyle(.secondary)
            }
            if let line = model.personaMessage { Text(line).font(.callout) }
        }
    }
}

struct DoneStep: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            StepHeader(title: "FERB is ready", lines: ["FERB lives in the notch at the top of your screen.",
                                                       "Three things to know:"])
            Text("• Double-tap the right Option key to open FERB, or click the notch.")
            Text("• Hold right Command + Option and speak to ask by voice.")
            Text("• Close the notch and keep working: FERB tells you when it needs you or finishes.")
        }
    }
}
```

- [ ] **Step 4: Rewrite `AppSetupController.swift`**

```swift
import AppKit
import SwiftUI
import NotchCore

enum AppName {
    static var current: String { Bundle.main.object(forInfoDictionaryKey: "CFBundleName") as? String ?? "FERB" }
}

/// FERB.app's setup (L27, L34 C). A first launch shows the native setup window: Welcome installs the background service
/// from this app (`ferb setup --service`, no questions), and every later step reads and acts through the daemon. After the
/// app is replaced by a new build, or when launchd doesn't have FERB running, launch updates the service quietly
/// (`ferb setup --service --update`) and shows the window only if that fails or a required grant is missing. Another
/// installation (a checkout, or FERB.app elsewhere) is replaced only after the owner says so, from the menu too. Closing
/// the window doesn't stop a service run (Set Up FERB… shows it again); quitting FERB stops it, and setup finishes
/// reinstalling the service first. A development build, which has no Python inside, never installs anything.
@MainActor
final class AppSetupController: NSObject, NSWindowDelegate {
    private let bundle = Bundle.main.bundleURL.resolvingSymlinksInPath()
    private var window: NSWindow?
    var onFinished: (() -> Void)?

    private var python: URL { bundle.appendingPathComponent("Contents/Resources/python/bin/python3.12") }
    private var agentExecutable: URL { bundle.appendingPathComponent("Contents/Helpers/FERB.app/Contents/MacOS/FERB") }
    private var launcher: URL { bundle.appendingPathComponent("Contents/Resources/bin/ferb") }
    private var agentPlist: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/LaunchAgents/com.ferb.daemon.plist")
    }

    private lazy var model = SetupModel(rpc: DaemonRPC(), runner: SetupCommand(launcher: launcher),
                                        app: SystemAppPermissions(), system: SystemOpener(),
                                        keys: KeychainAPIKeyStore(), service: DaemonService())

    var isPackaged: Bool {
        FileManager.default.isExecutableFile(atPath: launcher.path) && FileManager.default.fileExists(atPath: python.path)
            && FileManager.default.isExecutableFile(atPath: agentExecutable.path)
    }

    func startAtLaunch() {
        guard isPackaged else { return }
        let action = currentAction()
        Task { @MainActor in
            let done = action == .none ? await self.model.setupDone() : false
            switch AppSetupPlan.atLaunch(action, setupDone: done) {
            case .nothing: _ = await self.model.askForNotificationsOnce(defaults: .standard)   // owner decision 6
            case .quietUpdate: self.quietUpdate()
            case .window(let step):
                if case .confirmReplace(let current) = action, !self.confirmReplacing(current) { return }
                self.show(step, action: action)
            }
        }
    }

    /// Set Up FERB…: the window again; Welcome reinstalls only when this app's build isn't the one running.
    func runSetupFromMenu() {
        guard isPackaged else { return }
        let action = currentAction()
        if case .confirmReplace(let current) = action, !confirmReplacing(current) { return }
        show(.welcome, action: action)
    }

    /// FERB is quitting: stop a running setup command; it finishes reinstalling the background service first.
    func stop() { model.runner.cancel() }

    private func currentAction() -> AppSetupAction {
        let agent = NSDictionary(contentsOf: agentPlist) as? [String: Any]
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? ""
        return AppSetupPlan.decide(agent: agent, agentExecutable: agentExecutable.path, build: build,
                                   daemonRunning: daemonRunning())
    }

    private func quietUpdate() {
        model.serviceAction = .update
        model.runService { [weak self] ok in
            guard let self else { return }
            guard ok else { self.show(.welcome, action: .update); return }
            Task { @MainActor in
                if await self.model.needsPermissions() { self.show(.permissions, action: .none) }
                else { _ = await self.model.askForNotificationsOnce(defaults: .standard) }     // owner decision 6
            }
        }
    }

    /// Whether launchd has FERB's agent running now (`launchctl print` only reads launchd's state).
    private func daemonRunning() -> Bool {
        let launchctl = Process()
        launchctl.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        launchctl.arguments = ["print", "gui/\(getuid())/com.ferb.daemon"]
        let pipe = Pipe()
        launchctl.standardOutput = pipe
        launchctl.standardError = FileHandle.nullDevice
        do { try launchctl.run() } catch { return false }
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        launchctl.waitUntilExit()
        return launchctl.terminationStatus == 0 && String(decoding: data, as: UTF8.self).contains("state = running")
    }

    private func confirmReplacing(_ current: String) -> Bool {
        let alert = NSAlert()
        alert.messageText = "Set up FERB from this app?"
        alert.informativeText = "FERB's background service currently runs \(current). Setting up from this app " +
            "runs it from \(bundle.path) instead. Your data in ~/.ferb stays."
        alert.addButton(withTitle: "Set Up From This App")
        alert.addButton(withTitle: "Not Now")
        NSApp.activate(ignoringOtherApps: true)
        return alert.runModal() == .alertFirstButtonReturn
    }

    private func show(_ step: SetupStep, action: AppSetupAction) {
        if !model.serviceRunning { model.serviceAction = action }
        if window == nil {
            let created = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 560, height: 500),
                                   styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
            created.title = "Set Up \(AppName.current)"
            created.isReleasedWhenClosed = false
            created.delegate = self
            created.contentView = NSHostingView(rootView: SetupView(model: model))
            created.center()
            window = created
            model.onFinish = { [weak self] in
                self?.window?.close()
                self?.onFinished?()
            }
        }
        model.go(step)
        model.startPolling()
        NSApp.activate(ignoringOtherApps: true)          // the owner opened FERB or chose Set Up FERB…
        window?.makeKeyAndOrderFront(nil)
    }

    func windowWillClose(_ notification: Notification) {
        model.stopPolling()           // a service run keeps going: stopping it halfway could leave the service unloaded
    }
}
```

- [ ] **Step 5: Wire the app** — in `Application.swift` replace the block from `coordinator = PanelCoordinator()` to
  `appSetup.startAtLaunch()` with:

```swift
        coordinator = PanelCoordinator()
        if appSetup.isPackaged {
            coordinator?.onSetUp = { [appSetup] in appSetup.runSetupFromMenu() }
        }
        appSetup.onFinished = { [weak self] in self?.coordinator?.showFromMenu() }   // Done: into the notch
        coordinator?.start()
        appSetup.startAtLaunch()                 // packaged FERB.app only: the setup window or a quiet update
```

In `PanelCoordinator.swift`, delete the `var onCodexLogin` property, the
`if onCodexLogin != nil { addItem("Sign in to Codex…", …) }` line and `codexLoginFromMenu()` (Choose your AI replaces it).

- [ ] **Step 6: Run the tests and build**

Run: `cd ui/mac/FERBNotch && swift test && ./build.sh`
Expected: all tests PASS; `Built: …/build/FERB Notch.app`.

- [ ] **Step 7: Commit**

```bash
git add ui/mac/FERBNotch/Sources/FERBNotch/SetupView.swift ui/mac/FERBNotch/Sources/FERBNotch/AppSetupController.swift \
  ui/mac/FERBNotch/Sources/FERBNotch/Application.swift ui/mac/FERBNotch/Sources/FERBNotch/PanelCoordinator.swift \
  ui/mac/FERBNotch/Tests/NotchCoreTests/SetupModelTests.swift
git commit -m "feat(L34): the native setup window replaces the setup console; Choose your AI replaces Sign in to Codex"
```

---

### Task 9: Docs, the decision record, full verification and the owner's live check

**Files:**
- Modify: `README.md` (§ "FERB.app (disk image)", steps 3–4 and the update paragraph)
- Modify: `ARCHITECTURE.md` (§ 16, new subsection)
- Modify: `CHANGELOG.md`
- Modify: `docs/0.27_DECISIONS.toml` (`[L34]`)
- Modify: `docs/OWNER_ACCEPTANCE.md`

- [ ] **Step 1: README** — replace steps 3 and 4 of "FERB.app (disk image)" with:

```markdown
3. FERB opens its **Set Up FERB** window. **Set Up FERB** installs the background service from inside the app (a
   minute; a copy opened from the disk image is refused with how to fix it). Then: **Choose your AI** (Sign in with
   ChatGPT, which opens your browser and keeps the login in FERB's own Codex home `~/.ferb/codex-home`; Use Claude,
   your Claude subscription signed in to Claude Code on this Mac; or an API key, which goes straight into your
   Keychain), **Permissions** (each row's **Grant** opens the right System Settings pane and ticks itself when it's
   on; Full Disk Access shows FERB in Finder to drag into the list; optional rows: Input Monitoring, Paste from Other
   Apps for what you copy, Notifications, Microphone, Hearing app sound), **Browsers** (**Add to Chrome** opens FERB's
   Web Store page in that profile; FERB pairs a profile by itself when you're in it), **Getting to know you** (save,
   edit or skip what FERB found about the apps you use) and **Done**, which opens the notch.
4. Closing the window doesn't stop a service install; **Set Up FERB…** in FERB's menu bar shows the window again. An
   installation set up before this window existed is asked once, at launch, whether FERB may send notifications.
```

Replace the sentence beginning "After you replace FERB.app with a newer build, the next launch updates the background
service in the same window" through "(the window says which; Set Up FERB… walks through them)" with:

```markdown
After you replace FERB.app with a newer build, the next launch updates the background service quietly (and so does a
launch that finds it not running); the setup window opens only if that fails or a required permission is missing.
```

- [ ] **Step 2: ARCHITECTURE.md** — add under § 16 (create `### 16.15 FERB for everyone (L34)` if no other phase has;
  else add the subsection inside it):

```markdown
#### Phase C — the native setup window

FERB.app (`AppSetupController`) decides at launch from the installed LaunchAgent alone (`AppSetupPlan.decide`): a new
install or another installation (after the owner's yes) opens `SetupView`; a replaced build or a stopped service runs
`ferb setup --service --update` quietly and opens the window only on failure or a missing required grant
(`SetupModel.needsPermissions`); otherwise the window opens only while `setup.permissions.setup_done` is false
(`[setup] permissions_version` ≥ 27, set by `ferb setup` or `setup.finish`). Welcome runs the bundled `ferb setup
--service` (`service_setup`: `Installer.install()` from the bundle, the app scan, FERB Voice's pin, the browser install
requests; exit 0 once the daemon answers). Choose your AI uses `runtimes.status`/`runtimes.use`/`runtimes.check` and the
bundled `ferb setup --codex-login`; an API key goes on `security -i`'s stdin into the Keychain item `ferb setup --key`
writes (`KeychainAPIKeyStore`), never to the daemon. Permissions: the daemon's rows (`setup.permissions`, its own TCC
identity: the agent app `Contents/Helpers/FERB.app`) with `setup.permission_request` for macOS's prompt; Full Disk Access
is blocked until containment (CQ5) and revealed in Finder; Automation per running app through `automation.request`;
FERB.app's own Notifications and Microphone+Speech rows read and requested by FERB.app; FERB Voice's hearing row probed
only on entry. Browsers: `setup.browsers` (`autoinstall.read_states`, read only) and `setup.browser_open` (the Web Store
page in that profile, on the owner's click). Getting to know you: `setup.persona` with `chat.approve`/`chat.answer`. Done:
`setup.finish`, then the notch. Debugging: the journal kinds `setup.permission_requested`, `setup.browser_opened`,
`runtimes.changed`, `setup.finished`.
```

- [ ] **Step 3: CHANGELOG.md** — add at the top of the version list (or inside an existing L34 entry):

```markdown
## L34 FERB for everyone — Phase C: native setup — 2026-10-25 — checkout; owner live check pending

- FERB.app's setup window replaces the setup console: Welcome, Choose your AI, Permissions, Browsers, Getting to know
  you, Done; plain SwiftUI, styled later.
- `ferb setup --service [--update]` installs or updates the background service with no questions.
- New CLI-role RPCs: `setup.permissions`, `setup.permission_request`, `setup.finish`, `runtimes.status`,
  `runtimes.use`, `setup.browsers`, `setup.browser_open` (docs/RPC.md). `runtimes.check`'s detail is redacted.
- `ferb setup` asks macOS to list FERB (`setup.permission_request`) before it opens each pane; Sign in to Codex… left
  the menu bar (Choose your AI has it).
- Tests: tests/test_setup_rpcs.py, tests/test_permissions.py, tests/test_runtime_selection.py,
  tests/test_extension_autoinstall.py, tests/test_setup_flow.py, SetupModelsTests.swift, SetupModelTests.swift.
```

- [ ] **Step 4: Decision record** — in `docs/0.27_DECISIONS.toml`, if `[L34]` exists, append this phase's tests to its
  `tests`; otherwise add:

```toml
[L34]
milestone = "L"
tests = ["tests/test_setup_rpcs.py", "tests/test_permissions.py", "tests/test_runtime_selection.py",
         "tests/test_extension_autoinstall.py", "tests/test_setup_flow.py",
         "ui/mac/FERBNotch/Tests/NotchCoreTests/SetupModelsTests.swift",
         "ui/mac/FERBNotch/Tests/NotchCoreTests/SetupModelTests.swift"]
note = "Owner 2026-10-24/25 approved \"FERB for everyone\" (docs/superpowers/specs/2026-10-24-ferb-for-everyone-design.md): free items only, updates install themselves, two DMGs, the notch stays the main interface; 2026-10-25: \"all 7 parts in full, just design them bare min. focus on correct working\". One plan per phase (docs/superpowers/plans/2026-10-25-packaging-*.md)."
summary = "FERB for everyone: measured, shrunk two-architecture builds; one stable self-made signing certificate; a native setup window (Welcome, Choose your AI, Permissions, Browsers, Getting to know you, Done) through daemon setup RPCs; FERB.app's own notifications and a menu bar that shows the daemon's real state; the FERB window and Settings; launch at login; Sparkle updates."
```

- [ ] **Step 5: Owner acceptance** — append to `docs/OWNER_ACCEPTANCE.md`:

```markdown
## Native setup (L34 Phase C) — pending live check (2026-10-25)

Checkout validation is in the changelog entry; nothing here is installed or observed yet. Run on a fresh second macOS
user account with a built FERB.app (scripts/build-app.sh, scripts/build-dmg.sh), the owner watching; pause between
supervised actions and record what the owner saw.

- Open FERB.app straight from the disk image: Welcome's **Set Up FERB** fails with "drag FERB.app to Applications…" and
  stays on Welcome. Drag it to Applications, open it, **Set Up FERB**: the window moves to Choose your AI;
  `launchctl print gui/$(id -u)/com.ferb.daemon` shows it running.
- Choose your AI: **Sign in with ChatGPT** opens the browser; after signing in the row says Signed in and "FERB uses:
  ChatGPT (Codex)". Paste a disposable API key and Save: `security find-generic-password -s ferb -a api/<provider>` finds
  it; `ps -ax | grep security` during the save never shows the key; no file named like a history file appears in `~`.
- Permissions: each **Grant** opens the named pane; Accessibility, Screen Recording and Input Monitoring already list
  FERB; Full Disk Access shows FERB in Finder to drag in; after Screen Recording, **Restart FERB to check** ticks it.
  Notifications and Microphone show macOS's prompts for FERB.app itself. Paste from Other Apps: **Grant** shows macOS's
  paste alert once and opens Paste from Other Apps, where setting FERB to Allow ticks the row. Record anything that took
  focus unasked.
- On the owner's own Mac (already set up with `ferb setup`): the first launch of this build shows no setup window and
  macOS asks once whether FERB may send notifications; the next launch doesn't ask again.
- Browsers: **Add to Google Chrome** opens FERB's Web Store page in that profile; after adding and turning it on, the row
  reads On, then Paired once you're in that profile's window.
- Getting to know you: Save, Edit and Skip each close the question (the notch shows it answered too). Done opens the
  notch. Quit and reopen FERB.app: no window. Replace FERB.app with a newer build: launch updates quietly.
```

- [ ] **Step 6: Full verification**

Run: `nice -n 10 uv run pytest -q && uv run mypy ferb/core && git diff --check && (cd ui/mac/FERBNotch && swift test)`
Expected: all PASS, no whitespace errors. Report checkout validation, installation status (not installed) and the
owner's live results (pending) separately.

- [ ] **Step 7: Commit**

```bash
git add README.md ARCHITECTURE.md CHANGELOG.md docs/0.27_DECISIONS.toml docs/OWNER_ACCEPTANCE.md
git commit -m "docs(L34): Phase C native setup in README, ARCHITECTURE, CHANGELOG, the decision record and pending live checks"
```

---

### Task 10: Sign in with Claude, like Sign in with ChatGPT (owner decision 4)

Owner, 2026-10-25: "Build a Claude sign-in like Codex's." Today FERB signs in to Codex with `codex_login`
(`ferb/cli/setup.py`): the pinned Codex runs its own `codex login` with `CODEX_HOME=~/.ferb/codex-home`, opens the
browser itself and writes `auth.json` there; FERB never handles the token, and FERB's runs use that home only. This task
does the same for Claude.

**What the pinned CLI supports** (checked 2026-10-25 on the Claude Code that claude-agent-sdk 0.2.160 bundles,
`claude_agent_sdk/_bundled/claude`, version 2.1.283, run with a throwaway `HOME` and `CLAUDE_CONFIG_DIR`):
- `claude auth login [--claudeai|--console] [--email E] [--sso]` is not a terminal screen. It prints `Opening browser to
  sign in… If the browser didn't open, visit: <URL>`, then `Paste code here if prompted > `. It finishes on its own
  localhost callback, or on a code pasted on stdin when the page shows one. It writes the credential into the login
  Keychain itself, through `security -i` on stdin (`add-generic-password -U -a $USER -s "Claude Code-credentials-<first
  8 hex of sha256(CLAUDE_CONFIG_DIR)>" -X <hex>`), and runs read it with `security find-generic-password`.
- `claude auth status --json` prints `{"loggedIn": bool, "authMethod": "none"|…, "apiProvider": "firstParty", …}`.
- `claude setup-token` renders an interactive terminal screen (Ink), so FERB can't run it without a terminal, and it
  isn't used.

So FERB runs `auth login --claudeai` as its own child process, with `CLAUDE_CONFIG_DIR=~/.ferb/claude-home`. The owner
does the clicks in the browser, and FERB never types into a terminal (rule 10, D31). Claude runs then get the same
`CLAUDE_CONFIG_DIR` once that sign-in finished; until then they use Claude Code's default login, as today. When signing in
fails, or a future pin drops `auth login`, `why` gives the exact command for the owner to run in Terminal themselves
(`"<FERB.app>/Contents/Resources/bin/ferb" setup --claude-login`, or `ferb setup --claude-login` from a checkout).

**Files:**
- Create: `ferb/runtimes/claude_login.py`
- Modify: `ferb/runtimes/claude_sdk.py` (`_options`: `CLAUDE_CONFIG_DIR`)
- Modify: `ferb/cli/runtimes.py` (`status`: `signed_in`)
- Modify: `ferb/cli/setup.py` (`claude_login`), `ferb/cli/__init__.py` (`--claude-login`)
- Modify: `ferb/daemon/main.py` (`claude_sign_in` in `__init__`; `_runtimes_sign_in`; `_runtimes_status`;
  `_claude_signed_in`; `shutdown`)
- Modify: `ferb/daemon/auth.py`
- Create: `tests/support/fake_claude.py`, `tests/test_claude_login.py`
- Modify: `ui/mac/FERBNotch/Sources/NotchCore/SetupModels.swift` (`SignInStatus`, `RuntimeRow`, `AISnapshot`)
- Modify: `ui/mac/FERBNotch/Sources/FERBNotch/SetupModel.swift`, `ui/mac/FERBNotch/Sources/FERBNotch/SetupView.swift`
- Modify: `ui/mac/FERBNotch/Tests/NotchCoreTests/SetupModelsTests.swift`,
  `ui/mac/FERBNotch/Tests/NotchCoreTests/SetupModelTests.swift`
- Modify: `docs/RPC.md`, `README.md`, `ARCHITECTURE.md`, `CHANGELOG.md`, `docs/0.27_DECISIONS.toml`,
  `docs/OWNER_ACCEPTANCE.md`

**Interfaces:**
- Consumes: Task 3's `runtimes.status`, `runtimes.use`, `runtimes.check`, `ferb.cli.runtimes.status`; Task 7's
  `SetupModel.use(_:check:)` and `refreshAI()`; Task 8's `AIStepView`.
- Produces:
  - `ferb.runtimes.claude_login`: `SIGNED_IN`, `cli_binary() -> Path` (raises `Unsupported`), `home_dir(ferb_home)`,
    `claude_home(ferb_home) -> Path | None`, `prepare(ferb_home) -> Path`, `env(ferb_home, base=None) -> dict[str, str]`,
    `login_argv(cli)`, `status_argv(cli)`, `signed_in(status_output: str) -> bool`, `mark(ferb_home)`,
    `ferb_command() -> str`, `by_hand() -> str`, `sign_in_url(line) -> str | None`, `class ClaudeSignIn(ferb_home, cli,
    add_secret, done)` with `status()`, `async start()`, `async send_code(code)`, `async cancel()`, `async close()`.
  - `ferb.cli.setup.claude_login(env, run=subprocess.run) -> int`; `ferb setup --claude-login`.
  - RPC `runtimes.sign_in {runtime: "claude", code?, cancel?}` → `{state, url, why}`; `runtimes.status` rows gain
    `signed_in`, and the `claude` row gains `sign_in: {state, url, why}`; journal kinds `runtimes.sign_in {runtime,
    state}`, `runtimes.sign_in_finished {runtime, signed_in}`.
  - Swift: `SignInStatus` (`state`, `url`, `why`, `waiting`), `RuntimeRow.signedIn`/`.signIn`,
    `AISnapshot.signedIn(_:)`/`.signIn(_:)`; `SetupModel.claudeSigningIn`, `.claudeSignIn`, `signInWithClaude()`,
    `sendClaudeCode(_:)`, `cancelClaudeSignIn()`, `openClaudeSignInPage()`; view `ClaudeSignInRow(model:)`.

- [ ] **Step 1: Write the fake Claude Code** (`tests/support/fake_claude.py`)

```python
"""A stand-in for the pinned Claude Code's `auth login` and `auth status` (L34 C). It prints the sign-in page's address
the way Claude Code 2.1.283 does, takes a pasted code on stdin, "saves" a credential in its config dir as Claude Code
saves one in the Keychain, and even echoes it, so tests can check that FERB keeps none of it."""
import sys
from pathlib import Path

CREDENTIAL = "sk-ant-oat01-FAKE-CREDENTIAL-0123456789"
GOOD_CODE = "fake-auth-code-0123456789"
PAGE = "https://claude.ai/oauth/authorize?code=true&client_id=fake&state=fake-state"

SCRIPT = '''#!{python}
import json, os, pathlib, sys
home = pathlib.Path(os.environ["CLAUDE_CONFIG_DIR"])
saved = home / "fake-keychain-item"
args = sys.argv[1:]
if args[:2] == ["auth", "status"]:
    print(json.dumps({{"loggedIn": saved.exists(), "authMethod": "claude.ai" if saved.exists() else "none",
                      "apiProvider": "firstParty"}}))
    sys.exit(0)
if args == ["auth", "login", "--claudeai"]:
    if os.environ.get("ANTHROPIC_API_KEY") != "" or os.environ.get("CLAUDE_CODE_OAUTH_TOKEN") != "":
        print("a key or token from the environment would take over the login", flush=True)
        sys.exit(3)
    print("Opening browser to sign in... If the browser didn't open, visit: {page}", flush=True)
    sys.stdout.write("Paste code here if prompted > ")
    sys.stdout.flush()
    code = sys.stdin.readline().strip()
    if code != "{code}":
        print("Invalid code. Please make sure the full code was copied", flush=True)
        sys.exit(1)
    saved.write_text("{credential}")
    print("Login successful. Credential {credential}", flush=True)
    sys.exit(0)
print("unknown command", args, flush=True)
sys.exit(2)
'''


def fake_claude(folder: Path) -> Path:
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / "claude"
    path.write_text(SCRIPT.format(python=sys.executable, page=PAGE, code=GOOD_CODE, credential=CREDENTIAL))
    path.chmod(0o755)
    return path
```

- [ ] **Step 2: Write the failing Python tests** (`tests/test_claude_login.py`)

```python
"""L34 C (owner 2026-10-25: "Build a Claude sign-in like Codex's"): FERB's own Claude sign-in. The pinned Claude Code
signs in with FERB's Claude home and keeps the credential itself; FERB records only that it finished. A fake Claude Code
stands in for the real one: nothing here opens a browser or touches the Keychain."""
import asyncio
import io
import json
import subprocess
import time
from pathlib import Path
from typing import Any, Callable

import pytest

from ferb.cli import runtimes as runtimes_cli
from ferb.cli import setup as S
from ferb.cli.client import Client, RpcError
from ferb.core.errors import FerbError, Unsupported
from ferb.device.install import Installer
from ferb.runtimes import claude_login as CL
from ferb.runtimes.claude_sdk import ClaudeRuntime
from tests.conftest import RunningDaemon
from tests.support.fake_claude import CREDENTIAL, GOOD_CODE, PAGE, fake_claude


async def until_async(check: Callable[[], bool]) -> None:
    while not check():
        await asyncio.sleep(0.01)


def until(check: Callable[[], bool], seconds: float = 10) -> None:
    end = time.monotonic() + seconds
    while not check():
        if time.monotonic() > end:
            raise AssertionError("never happened")
        time.sleep(0.05)


def on_loop(daemon: RunningDaemon, fn: Callable[[], Any]) -> Any:
    async def go() -> Any:
        return fn()
    return daemon.run(go())


# ---------- the sign-in itself ----------

def test_a_pasted_code_finishes_the_sign_in_and_only_a_marker_is_left(tmp_path: Path) -> None:
    cli = fake_claude(tmp_path / "bin")
    secrets: list[str] = []
    finished: list[bool] = []

    async def run() -> dict[str, str]:
        sign_in = CL.ClaudeSignIn(tmp_path / "home", lambda: cli, secrets.append, finished.append)
        assert (await sign_in.start())["state"] == "waiting"
        await until_async(lambda: bool(sign_in.url))
        assert sign_in.url == PAGE
        await sign_in.send_code(f"  {GOOD_CODE} ")
        await until_async(lambda: sign_in.state != "waiting")
        return sign_in.status()
    assert asyncio.run(asyncio.wait_for(run(), 10)) == {"state": "signed_in", "url": "", "why": ""}
    assert finished == [True] and secrets == [GOOD_CODE]           # the code joined the redactor first
    home = tmp_path / "home" / "claude-home"
    assert CL.claude_home(tmp_path / "home") == home
    marker = (home / CL.SIGNED_IN).read_text()
    assert CREDENTIAL not in marker and GOOD_CODE not in marker
    assert home.stat().st_mode & 0o777 == 0o700


def test_a_wrong_code_fails_with_the_steps_to_sign_in_by_hand(tmp_path: Path) -> None:
    cli = fake_claude(tmp_path / "bin")
    finished: list[bool] = []

    async def run() -> dict[str, str]:
        sign_in = CL.ClaudeSignIn(tmp_path / "home", lambda: cli, lambda code: None, finished.append)
        await sign_in.start()
        await until_async(lambda: bool(sign_in.url))
        await sign_in.send_code("wrong-code")
        await until_async(lambda: sign_in.state != "waiting")
        return sign_in.status()
    status = asyncio.run(asyncio.wait_for(run(), 10))
    assert status["state"] == "failed" and "setup --claude-login" in status["why"]
    assert finished == [False] and CL.claude_home(tmp_path / "home") is None


def test_cancel_stops_the_waiting_sign_in(tmp_path: Path) -> None:
    cli = fake_claude(tmp_path / "bin")
    finished: list[bool] = []

    async def run() -> dict[str, str]:
        sign_in = CL.ClaudeSignIn(tmp_path / "home", lambda: cli, lambda code: None, finished.append)
        await sign_in.start()
        await until_async(lambda: bool(sign_in.url))
        status = await sign_in.cancel()
        assert sign_in._proc is None
        return status
    assert asyncio.run(asyncio.wait_for(run(), 10)) == {"state": "idle", "url": "", "why": ""}
    assert finished == [] and CL.claude_home(tmp_path / "home") is None


def test_a_code_needs_a_waiting_sign_in_and_is_one_word(tmp_path: Path) -> None:
    cli = fake_claude(tmp_path / "bin")

    async def run() -> None:
        sign_in = CL.ClaudeSignIn(tmp_path / "home", lambda: cli, lambda code: None, lambda ok: None)
        with pytest.raises(FerbError, match="waiting"):
            await sign_in.send_code(GOOD_CODE)
        await sign_in.start()
        with pytest.raises(FerbError, match="Paste the code"):
            await sign_in.send_code("two words")
        await sign_in.cancel()
    asyncio.run(asyncio.wait_for(run(), 10))


def test_a_missing_bundled_claude_code_says_how_to_sign_in_by_hand(tmp_path: Path) -> None:
    def missing() -> Path:
        raise Unsupported("the Claude Agent SDK's bundled Claude Code is missing")
    status = asyncio.run(CL.ClaudeSignIn(tmp_path, missing, lambda code: None, lambda ok: None).start())
    assert status["state"] == "failed" and "setup --claude-login" in status["why"]


def test_only_a_saved_subscription_login_counts_and_no_key_takes_over(tmp_path: Path) -> None:
    assert CL.signed_in('{"loggedIn": true, "authMethod": "claude.ai", "apiProvider": "firstParty"}')
    assert not CL.signed_in('{"loggedIn": false, "authMethod": "none"}')
    assert not CL.signed_in('{"loggedIn": true, "authMethod": "api_key"}')
    assert not CL.signed_in('{"loggedIn": true, "apiProvider": "bedrock"}')
    assert not CL.signed_in("Not logged in. Run claude auth login to authenticate.")
    env = CL.env(tmp_path, {"ANTHROPIC_API_KEY": "sk-x", "CLAUDE_CODE_OAUTH_TOKEN": "t", "PATH": "/usr/bin"})
    assert env["ANTHROPIC_API_KEY"] == env["CLAUDE_CODE_OAUTH_TOKEN"] == "" and env["PATH"] == "/usr/bin"
    assert env["CLAUDE_CONFIG_DIR"] == str(tmp_path / "claude-home")
    assert CL.login_argv(Path("/c")) == ["/c", "auth", "login", "--claudeai"]
    assert CL.status_argv(Path("/c")) == ["/c", "auth", "status", "--json"]
    assert CL.sign_in_url(f"If the browser didn't open, visit: {PAGE}") == PAGE


# ---------- runs, status and the terminal ----------

def test_runs_use_ferbs_own_claude_home_once_signed_in(tmp_path: Path) -> None:
    proxy = type("P", (), {"base_url": lambda self, run_id: "http://unused"})()
    rt = ClaudeRuntime("supervisor", proxy, str(tmp_path))                          # type: ignore[arg-type]
    assert "CLAUDE_CONFIG_DIR" not in rt._options("run", "").env      # no FERB sign-in yet: Claude Code's default
    CL.mark(tmp_path)
    assert rt._options("run", "").env["CLAUDE_CONFIG_DIR"] == str(tmp_path / "claude-home")


def test_status_says_which_runtimes_ferb_is_signed_in_to(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("ferb.runtimes.keys.has_api_key", lambda provider: provider == "openai")
    monkeypatch.setattr("ferb.runtimes.codex_exec.codex_binary", lambda *a, **k: Path("/x/codex"))
    rows = {r["name"]: r for r in runtimes_cli.status(tmp_path)["runtimes"]}
    assert (rows["claude"]["signed_in"], rows["codex"]["signed_in"], rows["openai"]["signed_in"]) == (False, False, True)
    CL.mark(tmp_path)
    assert {r["name"]: r for r in runtimes_cli.status(tmp_path)["runtimes"]}["claude"]["signed_in"] is True


def test_ferb_setup_claude_login_signs_in_from_a_terminal(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(CL, "cli_binary", lambda: Path("/fake/claude"))
    ran: list[tuple[list[str], str]] = []
    answer = {"loggedIn": True, "authMethod": "claude.ai", "apiProvider": "firstParty"}

    def run(argv: list[str], **kw: Any) -> subprocess.CompletedProcess[str]:
        ran.append((argv, kw["env"]["CLAUDE_CONFIG_DIR"]))
        out = json.dumps(answer) if argv[1:3] == ["auth", "status"] else ""
        return subprocess.CompletedProcess(argv, 0, out, "")
    out = io.StringIO()
    env = S.Env(Installer(tmp_path / "home"), lambda: None, input, out)   # type: ignore[arg-type]
    assert S.claude_login(env, run=run) == 0, out.getvalue()
    home = str(tmp_path / "home" / "claude-home")
    assert ran == [(["/fake/claude", "auth", "login", "--claudeai"], home),
                   (["/fake/claude", "auth", "status", "--json"], home)]
    assert CL.claude_home(tmp_path / "home") is not None and "Done" in out.getvalue()
    answer["loggedIn"] = False
    (tmp_path / "home" / "claude-home" / CL.SIGNED_IN).unlink()
    assert S.claude_login(env, run=run) == 1 and CL.claude_home(tmp_path / "home") is None


# ---------- through the daemon ----------

def test_claude_signs_in_through_the_daemon_and_nothing_secret_is_recorded(daemon: RunningDaemon, tmp_path: Path,
                                                                           monkeypatch: pytest.MonkeyPatch) -> None:
    cli = fake_claude(tmp_path / "bin")
    monkeypatch.setattr(CL, "cli_binary", lambda: cli)
    monkeypatch.setattr("ferb.runtimes.keys.has_api_key", lambda provider: False)
    d = daemon.daemon
    client = Client(daemon.sock)

    def claude_row() -> dict[str, Any]:
        return next(r for r in client.call("runtimes.status")["runtimes"] if r["name"] == "claude")
    try:
        assert client.call("runtimes.sign_in", {"runtime": "claude"})["state"] == "waiting"
        until(lambda: claude_row()["sign_in"]["url"] == PAGE)
        client.call("runtimes.sign_in", {"runtime": "claude", "code": GOOD_CODE})
        until(lambda: claude_row()["signed_in"] is True)
        assert claude_row()["sign_in"]["state"] == "signed_in"
        with pytest.raises(RpcError, match="Only Claude"):
            client.call("runtimes.sign_in", {"runtime": "codex"})
        with pytest.raises(RpcError, match="waiting"):
            client.call("runtimes.sign_in", {"runtime": "claude", "code": GOOD_CODE})   # nothing waits any more
    finally:
        client.close()
    recorded = on_loop(daemon, lambda: json.dumps(
        [dict(r) for r in d.db.conn.execute("SELECT * FROM journal")] +       # type: ignore[union-attr]
        [dict(r) for r in d.db.conn.execute("SELECT * FROM spans")], default=str))   # type: ignore[union-attr]
    assert CREDENTIAL not in recorded and GOOD_CODE not in recorded and PAGE not in recorded
    kinds = on_loop(daemon, lambda: [e.kind for e in d._journal().entries()])
    assert "runtimes.sign_in" in kinds and "runtimes.sign_in_finished" in kinds
    assert GOOD_CODE not in d._redact_text(f"the code was {GOOD_CODE}")       # the code joined the redactor
```

- [ ] **Step 3: Run them to verify they fail**

Run: `nice -n 10 uv run pytest -q tests/test_claude_login.py`
Expected: FAIL — `ImportError: cannot import name 'claude_login' from 'ferb.runtimes'`.

- [ ] **Step 4: Implement `ferb/runtimes/claude_login.py`**

```python
"""Owns FERB's own Claude sign-in (L34 C; owner 2026-10-25: "Build a Claude sign-in like Codex's"). Must not read,
keep, log or forward the credential, and never types into a terminal (rule 10, D31).

Like FERB's Codex sign-in (`codex login` with CODEX_HOME=~/.ferb/codex-home), the pinned Claude Code that the Claude
Agent SDK bundles runs its own non-interactive `auth login --claudeai` with CLAUDE_CONFIG_DIR=~/.ferb/claude-home, so
FERB's runs never load the owner's personal Claude Code settings. Claude Code opens the sign-in page in the browser,
gets the result on its localhost callback (or a code the owner pastes from the page), and writes the credential into
the login Keychain itself (`security -i` on stdin; item `Claude Code-credentials-<first 8 hex of sha256(config dir)>`).
FERB never sees it: runs started with the same CLAUDE_CONFIG_DIR read that saved login. FERB records only that a
sign-in finished (SIGNED_IN, a marker that holds no secret), after `auth status --json` said so. Checked on Claude Code
2.1.283 (claude-agent-sdk 0.2.160); its `setup-token` is an interactive terminal screen and isn't used."""

import asyncio
import json
import os
import re
from contextlib import suppress
from pathlib import Path
from typing import Callable

from ferb.core.errors import FerbError, Unsupported

SIGNED_IN = ".ferb-signed-in"
_URL = re.compile(r"https://\S+")


def cli_binary() -> Path:
    """The pinned Claude Code the SDK bundles (claude_agent_sdk/_bundled/claude)."""
    import claude_agent_sdk
    path = Path(claude_agent_sdk.__file__).parent / "_bundled" / "claude"
    if not path.is_file():
        raise Unsupported("the Claude Agent SDK's bundled Claude Code is missing")
    return path


def home_dir(ferb_home: Path) -> Path:
    return ferb_home / "claude-home"


def claude_home(ferb_home: Path) -> Path | None:
    """FERB's own Claude home once a sign-in there finished; None: runs use Claude Code's default login, as before."""
    home = home_dir(ferb_home)
    return home if (home / SIGNED_IN).is_file() else None


def prepare(ferb_home: Path) -> Path:
    home = home_dir(ferb_home)
    home.mkdir(parents=True, exist_ok=True)
    os.chmod(home, 0o700)
    return home


def env(ferb_home: Path, base: dict[str, str] | None = None) -> dict[str, str]:
    """Claude Code's environment for signing in: FERB's Claude home; no key or token in this environment can take over
    the login; telemetry and error reports off, as for runs."""
    out = dict(os.environ if base is None else base)
    out.update({"CLAUDE_CONFIG_DIR": str(home_dir(ferb_home)), "ANTHROPIC_API_KEY": "", "ANTHROPIC_AUTH_TOKEN": "",
                "CLAUDE_CODE_OAUTH_TOKEN": "", "DISABLE_TELEMETRY": "1", "DISABLE_ERROR_REPORTING": "1"})
    return out


def login_argv(cli: Path) -> list[str]:
    return [str(cli), "auth", "login", "--claudeai"]


def status_argv(cli: Path) -> list[str]:
    return [str(cli), "auth", "status", "--json"]


def signed_in(status_output: str) -> bool:
    """`auth status --json` says a Claude subscription login is saved there (an API key doesn't count: FERB's Claude
    runs refuse one)."""
    try:
        status = json.loads(status_output)
    except ValueError:
        return False
    return (isinstance(status, dict) and status.get("loggedIn") is True
            and status.get("apiProvider", "firstParty") == "firstParty"
            and "api" not in str(status.get("authMethod", "")).lower())


def mark(ferb_home: Path) -> None:
    (prepare(ferb_home) / SIGNED_IN).write_text(
        "FERB's own Claude sign-in finished; Claude Code keeps the credential in the login Keychain.\n")


def ferb_command() -> str:
    """The `ferb` command the owner can run: FERB.app's own launcher, or `ferb` from a checkout."""
    from ferb.device.install import BUNDLE_CLI
    from ferb.paths import app_bundle
    bundle = app_bundle()
    return f'"{bundle / BUNDLE_CLI}"' if bundle is not None else "ferb"


def by_hand() -> str:
    """The exact steps to sign in without FERB.app. The owner runs them; FERB never types into a terminal (rule 10)."""
    return f"To sign in by hand, run this in Terminal yourself: {ferb_command()} setup --claude-login"


def sign_in_url(line: str) -> str | None:
    found = _URL.search(line)
    return found.group(0).rstrip(".,)") if found else None


class ClaudeSignIn:
    """One sign-in at a time, run by the daemon for FERB.app (`runtimes.sign_in`): its state, the page to open when the
    browser didn't, and the owner's pasted code. Claude Code's output is read only for that page's address; nothing it
    prints is kept, logged or journaled. `done(signed_in)` is told when a sign-in ends (not when it is cancelled)."""

    def __init__(self, ferb_home: Path, cli: Callable[[], Path], add_secret: Callable[[str], None],
                 done: Callable[[bool], None]) -> None:
        self.ferb_home, self.cli, self.add_secret, self.done = ferb_home, cli, add_secret, done
        self.state, self.url, self.why = "idle", "", ""
        self._proc: asyncio.subprocess.Process | None = None
        self._follow_task: asyncio.Task[None] | None = None
        self._cancelled = False

    def status(self) -> dict[str, str]:
        return {"state": self.state, "url": self.url, "why": self.why}

    async def start(self) -> dict[str, str]:
        if self.state == "waiting":
            return self.status()
        try:
            cli = self.cli()
        except Unsupported as e:
            self.state, self.url, self.why = "failed", "", f"FERB can't sign in to Claude here ({e}). {by_hand()}"
            return self.status()
        prepare(self.ferb_home)
        self.state, self.url, self.why, self._cancelled = "waiting", "", "", False
        self._proc = await asyncio.create_subprocess_exec(
            *login_argv(cli), env=env(self.ferb_home), stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT, start_new_session=True)
        self._follow_task = asyncio.create_task(self._follow(cli, self._proc))
        return self.status()

    async def _follow(self, cli: Path, proc: asyncio.subprocess.Process) -> None:
        assert proc.stdout is not None
        async for raw in proc.stdout:                  # read for the page's address only; nothing else is kept
            url = sign_in_url(raw.decode("utf-8", "replace"))
            if url and not self.url:
                self.url = url
        code = await proc.wait()
        self._proc = None
        if self._cancelled:
            return
        ok = code == 0 and await self._confirmed(cli)
        if ok:
            mark(self.ferb_home)
        self.state, self.url = ("signed_in" if ok else "failed"), ""
        self.why = "" if ok else f"Claude didn't confirm the sign-in. Try again. {by_hand()}"
        self.done(ok)

    async def _confirmed(self, cli: Path) -> bool:
        proc = await asyncio.create_subprocess_exec(
            *status_argv(cli), env=env(self.ferb_home), stdin=asyncio.subprocess.DEVNULL,
            stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.DEVNULL)
        out, _ = await proc.communicate()
        return signed_in(out.decode("utf-8", "replace"))

    async def send_code(self, code: str) -> dict[str, str]:
        """The code the sign-in page shows when its own redirect couldn't finish: to Claude Code's stdin only."""
        code = code.strip()
        proc = self._proc
        if self.state != "waiting" or proc is None or proc.stdin is None:
            raise FerbError("No Claude sign-in is waiting for a code; start it again")
        if not code or any(c.isspace() for c in code):
            raise FerbError("Paste the code the sign-in page shows")
        self.add_secret(code)                          # rule 14: redacted wherever anything echoes it
        proc.stdin.write(code.encode() + b"\n")
        await proc.stdin.drain()
        return self.status()

    async def cancel(self) -> dict[str, str]:
        self._cancelled = True
        proc = self._proc
        if proc is not None and proc.returncode is None:
            proc.terminate()
        if self._follow_task is not None:
            with suppress(Exception):
                await self._follow_task
        self.state, self.url, self.why = "idle", "", ""
        return self.status()

    async def close(self) -> None:
        """Daemon shutdown: a sign-in still waiting stops."""
        if self.state == "waiting":
            await self.cancel()
```

- [ ] **Step 5: Runs, status and the terminal command**

`ferb/runtimes/claude_sdk.py`: add `from pathlib import Path` to the imports. In `_options`, after the `env = {…}` dict:

```python
        from ferb.runtimes.claude_login import claude_home
        own = claude_home(Path(self.ferb_home))       # L34 C: FERB's own Claude sign-in, once it finished
        if own is not None:
            env["CLAUDE_CONFIG_DIR"] = str(own)
```

`ferb/cli/runtimes.py`: add above `status`:

```python
def _signed_in(name: str, home: Path, usable: bool) -> bool:
    """FERB's own Codex or Claude sign-in finished, or (a direct API) the provider's key is in the Keychain."""
    if name == "codex":
        from ferb.runtimes.codex_exec import codex_home
        return codex_home(home) is not None
    if name == "claude":
        from ferb.runtimes.claude_login import claude_home
        return claude_home(home) is not None
    return usable
```

and in `status`, the row becomes:

```python
        rows.append({"name": name, "usable": ok, "fix": fix, "signed_in": _signed_in(name, home, ok),
                     "model": str(cfg.get("models", name) or info.default_model or "")})
```

`ferb/cli/setup.py`, after `codex_login`:

```python
def claude_login(env: Env, run: Callable[..., Any] = subprocess.run) -> int:
    """FERB's own Claude sign-in from a terminal (owner 2026-10-25: "Build a Claude sign-in like Codex's"): the pinned
    Claude Code the SDK bundles runs `auth login` with FERB's Claude home (~/.ferb/claude-home). Its browser page and any
    pasted code go to Claude Code directly (this terminal), which keeps the credential in the login Keychain; FERB records
    only that it finished."""
    from ferb.runtimes import claude_login as cl
    try:
        cli = cl.cli_binary()
    except Exception as e:
        env.say(f"FERB's pinned Claude Code isn't installed: {e}")
        return 1
    cl.prepare(env.inst.home)
    env.say("Signing in FERB's own Claude (a browser window opens)…")
    run(cl.login_argv(cli), env=cl.env(env.inst.home))
    status = run(cl.status_argv(cli), env=cl.env(env.inst.home), capture_output=True, text=True)
    if not cl.signed_in(status.stdout or ""):
        env.say("Not signed in; nothing changed.")
        return 1
    cl.mark(env.inst.home)
    env.say("Done: FERB now uses its own Claude sign-in.")
    return 0
```

`ferb/cli/__init__.py`, after the `--codex-login` argument:

```python
    sp.add_argument("--claude-login", action="store_true",
                    help="sign in FERB's own Claude (its own home: your personal Claude Code settings stay out)")
```

and in the setup dispatch, after the `codex_login` line:

```python
            if args.cmd == "setup" and args.claude_login:
                return setup_cmd.claude_login(env)
```

- [ ] **Step 6: The daemon side**

`ferb/daemon/auth.py`: add `"runtimes.sign_in",` to the `Role.CLI` set.

`ferb/daemon/main.py`, at the end of `__init__` (after Task 2's seams):

```python
        # L34 C (owner 2026-10-25): FERB's own Claude sign-in for FERB.app; `cli_binary` is looked up when it starts
        from ferb.runtimes import claude_login as _claude_login
        self.claude_sign_in = _claude_login.ClaudeSignIn(
            self.home, lambda: _claude_login.cli_binary(), self._add_secret_value, self._claude_signed_in)
```

Registration (after Task 3's lines): `self.server.unary['runtimes.sign_in'] = self._runtimes_sign_in`.

In `shutdown()`, after `await self.persona.close()`'s `if` block: `await self.claude_sign_in.close()`.

Replace Task 3's `_runtimes_status` body with:

```python
        from ferb.cli import runtimes as runtimes_cli
        out = await asyncio.to_thread(runtimes_cli.status, self.home)
        for row in out["runtimes"]:
            if row["name"] == "claude":
                row["sign_in"] = self.claude_sign_in.status()      # L34 C: a sign-in under way, or how it ended
        return out
```

Add next to it:

```python
    async def _runtimes_sign_in(self, session: Session, params: dict[str, Any]) -> dict[str, Any]:
        """L34 C (owner 2026-10-25: "Build a Claude sign-in like Codex's"): FERB's own Claude sign-in for FERB.app.
        `{runtime: "claude"}` starts it (or reports the one waiting); `code` passes the code the sign-in page shows to
        the waiting sign-in; `cancel: true` stops it. Journaled without the code or anything Claude Code printed."""
        if params.get("runtime") != "claude":
            raise FerbError("Only Claude signs in here; ChatGPT signs in with Sign in with ChatGPT "
                            "(`ferb setup --codex-login`)")
        if params.get("cancel") is True:
            return await self.claude_sign_in.cancel()
        if "code" in params:
            return await self.claude_sign_in.send_code(str(params["code"]))
        status = await self.claude_sign_in.start()
        self._journal().append("runtimes.sign_in", {"runtime": "claude", "state": status["state"]})
        return status

    def _claude_signed_in(self, ok: bool) -> None:
        """FERB's own Claude sign-in ended: journaled, never with the credential or the code."""
        self._available_cache.pop("claude", None)
        self._journal().append("runtimes.sign_in_finished", {"runtime": "claude", "signed_in": ok})
```

- [ ] **Step 7: Run the Python tests**

Run: `nice -n 10 uv run pytest -q tests/test_claude_login.py tests/test_runtime_selection.py tests/test_claude_prompt_cache.py tests/test_checker_effort.py tests/test_claude_steering.py tests/test_auth.py tests/test_setup_flow.py`
Expected: PASS.

- [ ] **Step 8: Write the failing Swift tests**

Append to `SetupModelsTests` (NotchCore):

```swift
    func testAIReplyCarriesEachSignIn() {
        let snapshot = AISnapshot(reply: [
            "roles": ["orchestrator": "claude", "worker": "claude", "checker": "claude"],
            "runtimes": [["name": "claude", "usable": true, "fix": "", "model": "", "signed_in": false,
                          "sign_in": ["state": "waiting", "url": "https://claude.ai/oauth/authorize?x=1", "why": ""]],
                         ["name": "codex", "usable": true, "fix": "", "model": "", "signed_in": true]]])!
        XCTAssertEqual(snapshot.signIn("claude")?.state, "waiting")
        XCTAssertEqual(snapshot.signIn("claude")?.url?.host, "claude.ai")
        XCTAssertTrue(snapshot.signIn("claude")!.waiting)
        XCTAssertNil(snapshot.signIn("codex"))
        XCTAssertTrue(snapshot.signedIn("codex"))
        XCTAssertFalse(snapshot.signedIn("claude"))
        XCTAssertNil(SignInStatus(["state": "idle", "url": "", "why": ""])?.url)
        XCTAssertNil(SignInStatus(nil))
    }
```

Append inside `SetupModelTests` (FERBNotch), before its closing brace:

```swift
    private func aiReply(claude signIn: [String: Any], signedIn: Bool) -> [String: Any] {
        ["roles": ["orchestrator": "codex", "worker": "codex", "checker": "codex"],
         "runtimes": [["name": "codex", "usable": true, "fix": "", "model": "gpt-5.6-sol", "signed_in": true],
                      ["name": "claude", "usable": true, "fix": "", "model": "", "signed_in": signedIn,
                       "sign_in": signIn]]]
    }

    func testSignInWithClaudeRunsTheDaemonsSignInThenChoosesClaude() async {
        let rig = SetupRig()
        rig.rpc.replies["runtimes.sign_in"] = ["state": "waiting", "url": "", "why": ""]
        rig.rpc.replies["runtimes.status"] = aiReply(claude: ["state": "waiting",
                                                              "url": "https://claude.ai/oauth/authorize?x=1", "why": ""],
                                                     signedIn: false)
        rig.rpc.replies["runtimes.check"] = ["ok": true, "detail": "answered"]
        let model = rig.model()
        await model.signInWithClaude()
        XCTAssertEqual(rig.rpc.params(of: "runtimes.sign_in").first?["runtime"] as? String, "claude")
        XCTAssertTrue(model.claudeSigningIn)
        model.openClaudeSignInPage()
        XCTAssertEqual(rig.opener.opened.last?.host, "claude.ai")
        await model.sendClaudeCode("  fake-auth-code-0123456789 \n")
        XCTAssertEqual(rig.rpc.params(of: "runtimes.sign_in").last?["code"] as? String, "fake-auth-code-0123456789")
        let state = Mirror(reflecting: model).children.map { String(describing: $0.value) }.joined()
        XCTAssertFalse(state.contains("fake-auth-code-0123456789"))          // the code isn't kept in the window
        rig.rpc.replies["runtimes.status"] = aiReply(claude: ["state": "signed_in", "url": "", "why": ""], signedIn: true)
        await model.refreshAI()
        XCTAssertFalse(model.claudeSigningIn)
        XCTAssertEqual(rig.rpc.params(of: "runtimes.use").last?["runtime"] as? String, "claude")
        XCTAssertEqual(model.aiMessage, "Claude answered. FERB uses it now.")
    }

    func testAFailedClaudeSignInShowsTheStepsToDoItByHand() async {
        let rig = SetupRig()
        let why = "Claude didn't confirm the sign-in. Try again. To sign in by hand, run this in Terminal yourself: " +
            "\"/Applications/FERB.app/Contents/Resources/bin/ferb\" setup --claude-login"
        rig.rpc.replies["runtimes.sign_in"] = ["state": "waiting", "url": "", "why": ""]
        let model = rig.model()
        await model.signInWithClaude()
        rig.rpc.replies["runtimes.status"] = aiReply(claude: ["state": "failed", "url": "", "why": why], signedIn: false)
        await model.refreshAI()
        XCTAssertFalse(model.claudeSigningIn)
        XCTAssertEqual(model.aiMessage, why)
        XCTAssertTrue(rig.rpc.params(of: "runtimes.use").isEmpty)
    }

    func testCancelStopsTheClaudeSignIn() async {
        let rig = SetupRig()
        rig.rpc.replies["runtimes.sign_in"] = ["state": "waiting", "url": "", "why": ""]
        let model = rig.model()
        await model.signInWithClaude()
        await model.cancelClaudeSignIn()
        XCTAssertEqual(rig.rpc.params(of: "runtimes.sign_in").last?["cancel"] as? Bool, true)
        XCTAssertFalse(model.claudeSigningIn)
    }
```

- [ ] **Step 9: Run them to verify they fail**

Run: `cd ui/mac/FERBNotch && swift test --filter "SetupModelsTests|SetupModelTests"`
Expected: FAIL to compile — `cannot find 'SignInStatus' in scope`, `value of type 'SetupModel' has no member
'signInWithClaude'`.

- [ ] **Step 10: Implement the Swift side**

`SetupModels.swift`: replace `RuntimeRow` with:

```swift
/// A sign-in the daemon runs for a runtime (`runtimes.sign_in`): idle, waiting (for the browser or a pasted code),
/// signed_in, or failed (`why` gives the exact steps to sign in by hand).
public struct SignInStatus: Equatable {
    public let state: String
    public let url: URL?
    public let why: String

    public init?(_ value: Any?) {
        guard let row = value as? [String: Any], let state = row["state"] as? String else { return nil }
        self.state = state
        url = (row["url"] as? String).flatMap { $0.isEmpty ? nil : URL(string: $0) }
        why = row["why"] as? String ?? ""
    }

    public var waiting: Bool { state == "waiting" }
}

public struct RuntimeRow: Equatable, Identifiable {
    public var id: String { name }
    public let name: String
    public let usable: Bool
    public let fix: String
    public let model: String
    public let signedIn: Bool
    public let signIn: SignInStatus?
}
```

In `AISnapshot.init?(reply:)`, the row closure returns:

```swift
            return RuntimeRow(name: name, usable: row["usable"] as? Bool ?? false, fix: row["fix"] as? String ?? "",
                              model: row["model"] as? String ?? "", signedIn: row["signed_in"] as? Bool ?? false,
                              signIn: SignInStatus(row["sign_in"]))
```

and add to `AISnapshot`:

```swift
    public func signedIn(_ name: String) -> Bool { runtimes.first { $0.name == name }?.signedIn ?? false }
    public func signIn(_ name: String) -> SignInStatus? { runtimes.first { $0.name == name }?.signIn }
```

`SetupModel.swift`: add next to `signingIn`:

```swift
    @Published private(set) var claudeSigningIn = false
```

replace `refreshAI()` with:

```swift
    func refreshAI() async {
        do {
            ai = AISnapshot(reply: try await rpc.call("runtimes.status", params: [:]))
            clearNotAnswering()
        } catch {
            message = Self.notAnswering
            return
        }
        guard claudeSigningIn, let status = claudeSignIn, !status.waiting else { return }
        claudeSigningIn = false                        // the daemon's Claude sign-in ended
        if status.state == "signed_in" { await use("claude", check: true) }
        else { aiMessage = status.why.isEmpty ? "Not signed in to Claude." : status.why }
    }
```

and add after `useClaude()`:

```swift
    var claudeSignIn: SignInStatus? { ai?.signIn("claude") }

    /// Sign in with Claude (owner 2026-10-25): the daemon runs the pinned Claude Code's own sign-in in the browser; the
    /// credential stays with Claude Code in the Keychain. Polling (`refreshAI`) sees it end, then FERB uses Claude.
    func signInWithClaude() async {
        do {
            let status = SignInStatus(try await rpc.call("runtimes.sign_in", params: ["runtime": "claude"]))
            claudeSigningIn = status?.waiting == true
            aiMessage = claudeSigningIn ? "Finish signing in to Claude in the browser window that opened."
                : (status?.why ?? "FERB couldn't start the Claude sign-in.")
        } catch { aiMessage = error.localizedDescription }
        await refreshAI()
    }

    /// The code the sign-in page shows when its own redirect couldn't finish; it goes to the daemon's waiting sign-in
    /// and is never kept here.
    func sendClaudeCode(_ code: String) async {
        let trimmed = code.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        do { _ = try await rpc.call("runtimes.sign_in", params: ["runtime": "claude", "code": trimmed]) }
        catch { aiMessage = error.localizedDescription }
    }

    func cancelClaudeSignIn() async {
        _ = try? await rpc.call("runtimes.sign_in", params: ["runtime": "claude", "cancel": true])
        claudeSigningIn = false
        aiMessage = nil
        await refreshAI()
    }

    /// The owner's click when the browser didn't open by itself.
    func openClaudeSignInPage() {
        if let url = claudeSignIn?.url { system.open(url) }
    }
```

`SetupView.swift`: in `AIStepView`, replace the `HStack` that holds **Use Claude** with `ClaudeSignInRow(model: model)`,
and add:

```swift
struct ClaudeSignInRow: View {
    @ObservedObject var model: SetupModel
    @State private var code = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Button(model.claudeSigningIn ? "Waiting for your browser…" : "Sign in with Claude") {
                    Task { await model.signInWithClaude() }
                }
                .disabled(model.claudeSigningIn)
                if model.claudeSigningIn { Button("Cancel") { Task { await model.cancelClaudeSignIn() } } }
                if model.ai?.signedIn("claude") == true && !model.claudeSigningIn {
                    Text("Signed in").foregroundStyle(.secondary)
                    if model.ai?.current != "claude" { Button("Use Claude") { Task { await model.useClaude() } } }
                }
            }
            if model.claudeSigningIn {
                if model.claudeSignIn?.url != nil {
                    Button("Open the sign-in page") { model.openClaudeSignInPage() }
                }
                HStack {
                    SecureField("Code from the page (only if it shows one)", text: $code)
                    Button("Send") {
                        let entered = code
                        code = ""                              // the field never keeps it
                        Task { await model.sendClaudeCode(entered) }
                    }
                    .disabled(code.isEmpty)
                }
            }
        }
    }
}
```

- [ ] **Step 11: Run the Swift tests and build**

Run: `cd ui/mac/FERBNotch && swift test && ./build.sh`
Expected: all PASS (including `testEveryStepDrawsWithoutTheDaemon`); `Built: …`.

- [ ] **Step 12: Docs**

`docs/RPC.md`, L34 table: change the `runtimes.status` row's result to `{roles: {orchestrator, worker, checker},
runtimes: [{name, usable, fix, model, signed_in}]}`, with the `claude` row also carrying `sign_in: {state, url, why}`, and
add:

```markdown
| `runtimes.sign_in` | `{runtime: "claude", code?: str, cancel?: true}` | `{state: "idle"\|"waiting"\|"signed_in"\|"failed", url, why}`. Starts FERB's own Claude sign-in (or reports the one waiting): the pinned Claude Code runs `auth login --claudeai` with `CLAUDE_CONFIG_DIR=~/.ferb/claude-home`, opens the sign-in page itself and keeps the credential in the login Keychain itself, so FERB never holds it. `url` is the page to open when the browser didn't. `code` passes the code the page shows to the waiting sign-in; it is added to the redactor and never journaled. `cancel` stops it. A failure's `why` names the exact command to run by hand (`ferb setup --claude-login`). Journaled `runtimes.sign_in` and `runtimes.sign_in_finished {runtime, signed_in}`. Any other runtime is refused |
```

`README.md`, in the setup step written in Task 9, replace "Use Claude, your Claude subscription signed in to Claude Code
on this Mac" with "Sign in with Claude, which opens your browser and keeps the login in FERB's own Claude home
`~/.ferb/claude-home` (Claude Code keeps the credential in your Keychain)". Add after the `ferb setup --codex-login`
mention in the source-install section (or with the other `ferb setup` flags): "`ferb setup --claude-login` signs in
FERB's own Claude from a terminal."

`ARCHITECTURE.md`, Phase C subsection, append: "Sign in with Claude (owner 2026-10-25): `runtimes.sign_in` runs
`ClaudeSignIn` (`ferb/runtimes/claude_login.py`), the pinned Claude Code's own `auth login --claudeai` with
`CLAUDE_CONFIG_DIR=~/.ferb/claude-home`; Claude Code writes the credential to the login Keychain itself; after `auth status
--json` confirms it, FERB writes the marker `.ferb-signed-in`, and Claude runs (`ClaudeRuntime._options`) then get that
`CLAUDE_CONFIG_DIR`. `ferb setup --claude-login` is the same from a terminal. Journal: `runtimes.sign_in`,
`runtimes.sign_in_finished`."

`CHANGELOG.md`, Phase C entry, add: "- Sign in with Claude, like Sign in with ChatGPT (owner, 2026-10-25): the pinned
Claude Code's own `auth login` with FERB's Claude home; the credential stays with Claude Code in the Keychain.
`runtimes.sign_in`, `runtimes.status` `signed_in`, `ferb setup --claude-login`. Tests: tests/test_claude_login.py."

`docs/0.27_DECISIONS.toml`: add `"tests/test_claude_login.py"` to `[L34]`'s `tests`.

`docs/OWNER_ACCEPTANCE.md`, the Phase C section, add:

```markdown
- Sign in with Claude (needs a Claude subscription): the browser opens Claude's sign-in page; after approving, the row
  says Signed in, "FERB uses: Claude" and the check answers. `security find-generic-password -s "Claude
  Code-credentials-$(printf %s ~/.ferb/claude-home | shasum -a 256 | cut -c1-8)"` finds the item (no `-w`: the value
  stays unread); `grep -r "sk-ant" ~/.ferb/logs` finds nothing. Cancel during a second sign-in stops it.
```

- [ ] **Step 13: Full verification**

Run: `nice -n 10 uv run pytest -q && uv run mypy ferb/core && git diff --check && (cd ui/mac/FERBNotch && swift test)`
Expected: all PASS.

- [ ] **Step 14: Commit**

```bash
git add ferb/runtimes/claude_login.py ferb/runtimes/claude_sdk.py ferb/cli/runtimes.py ferb/cli/setup.py \
  ferb/cli/__init__.py ferb/daemon/main.py ferb/daemon/auth.py tests/support/fake_claude.py tests/test_claude_login.py \
  ui/mac/FERBNotch/Sources/NotchCore/SetupModels.swift ui/mac/FERBNotch/Sources/FERBNotch/SetupModel.swift \
  ui/mac/FERBNotch/Sources/FERBNotch/SetupView.swift ui/mac/FERBNotch/Tests/NotchCoreTests/SetupModelsTests.swift \
  ui/mac/FERBNotch/Tests/NotchCoreTests/SetupModelTests.swift docs/RPC.md README.md ARCHITECTURE.md CHANGELOG.md \
  docs/0.27_DECISIONS.toml docs/OWNER_ACCEPTANCE.md
git commit -m "feat(L34): Sign in with Claude like Sign in with ChatGPT: the pinned Claude Code's own login in FERB's Claude home (rule 14)"
```
