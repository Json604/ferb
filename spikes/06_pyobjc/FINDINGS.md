# Spike 6 — pyobjc: frontmost app, window titles, Chrome AX tree, TCC checks

Status: done (2026-09-28). 10/10 tests pass, 5 runs in a row. Chrome 154.0.8037.57, Brave 151.1.93.136,
macOS 26.6.2, pyobjc (ApplicationServices, Quartz, Cocoa) + `ctypes` for Apple Events permission.

Run: `uv run --python 3.12 --no-project --with pyobjc-framework-ApplicationServices --with pyobjc-framework-Quartz
--with pyobjc-framework-Cocoa --with pytest pytest -q test_spike.py`

Browsers run as separate instances on temporary profiles showing a local page; nothing is brought to the front
and no permission dialog is triggered (every check passes `prompt/askUserIfNeeded = false`).

## Results

- **Frontmost app**: `NSWorkspace.frontmostApplication()` — no permission needed.
- **Window titles** without activating the app: Accessibility (`AXWindows` → `AXTitle`, e.g.
  `"ferb-ax-probe - Google Chrome"`) and CoreGraphics (`kCGWindowName`, page title only). CoreGraphics withholds
  other apps' titles without Screen Recording — the test checks the status matches what is visible.
- **Chrome/Brave web AX tree**: built **on demand for an AX client**; `AXManualAccessibility` is **no longer
  supported** (`AXUIElementSetAttributeValue` → `-25205`, attribute unsupported) in Chrome 154 and Brave 151.
  The first query sometimes returns an empty tree; the next one (~0.2 s later) is populated (measured over 5
  fresh launches each: Chrome 1/5, Brave 4/5 empty on the first query). Rule: if empty, re-query for up to ~1–2 s.
- **TCC status, no prompts**:
  - Accessibility: `AXIsProcessTrustedWithOptions({prompt: false})`.
  - Screen Recording: `CGPreflightScreenCaptureAccess()`.
  - Automation (per target app): `AEDeterminePermissionToAutomateTarget(bundle id, '****', '****', false)` via
    `ctypes` (not exposed by pyobjc): `0` granted, `-1743` denied, `-1744` not yet asked, `-600` target not running
    (must be running to check).
  - Full Disk Access: no API; probe-read `~/Library/Application Support/com.apple.TCC/TCC.db`.
  - Observed on this machine for this process: Accessibility ✓, Screen Recording ✓, Full Disk Access ✗,
    Automation Chrome ✓, Brave / Finder not yet asked.

## Important for build step 3 (`device/permissions.py`, `ferb doctor`)

- **TCC grants belong to the "responsible" app**, not to FERB: here the grants are VS Code's (Claude Code runs in
  its terminal). A launchd-started daemon is its own responsible process with **no grants** — the section 8 rule
  (self-test through the launchd-started process) is essential; spike 8 should confirm how grants attach to it.
- Automation status can only be checked while the target app runs; `doctor` must report "not running" distinctly.
- Notifications permission (section 8) was not checked: `UNUserNotificationCenter` needs a bundled app. Whether
  FERB shows notifications at all in this version (section 10.2's "Sending… Undo") is an open owner decision,
  since CLAUDE.md makes the CLI the only interface.
