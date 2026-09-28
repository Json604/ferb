# Spike 5 — Safari extension in FERB.app (PARKED, 2026-09-28)

Status: **parked by the owner** (doesn't use Safari). Built and partly checked; the Safari-side tests never ran.
This branch (`spike-5-safari`) keeps the work; `main` does not include it.

## Built

- `ext/` — Safari web extension (MV2, persistent background page): background-tab `open`, sanitized `snapshot`
  (password/OTP values redacted), script `click` / `set_value` by snapshot element id, `activate`, `close`.
  Spike-only control channel: long-poll to `http://127.0.0.1:47811` (spike 7 was to replace it with
  `SafariWebExtensionHandler` → daemon socket).
- `xcode/` — generated with `xcrun safari-web-extension-packager --macos-only --swift`, then stripped to a
  **no-UI container** (CLAUDE.md): storyboard, view controller and HTML resources removed, `LSUIElement = YES`,
  `main()` returns immediately. App bundle id fixed to `com.ferb.spike5` (the template's app id wasn't a prefix of
  the extension id `com.ferb.spike5.Extension`).
- Build: `xcodebuild … CODE_SIGN_STYLE=Manual CODE_SIGN_IDENTITY=- DEVELOPMENT_TEAM=` (ad-hoc "Sign to Run
  Locally"); needs Safari's "Allow unsigned extensions" (resets on every Safari quit).
- `safari_run.py` — harness (HTTP long-poll) + AX/real-input fallback helpers (pyobjc).
- `test_spike.py` — connect/version, background-tab snapshot, script click on Wikipedia search, AX refusal when
  the focused window isn't the target tab, AX + real keystrokes on the same site.

## Checked

- Xcode 26.3 builds it; launching the app exits immediately with no window/Dock icon; `pluginkit` lists
  `com.ferb.spike5.Extension` after one launch (registration then removed when parked).
- This process has Accessibility permission (`AXIsProcessTrusted`).

## Untested (never ran against Safari)

- Enabling the extension in a spare Safari profile; website-access prompts; whether the persistent background
  page survives Safari's lifecycle; the HTTP long-poll from the extension to 127.0.0.1.
- DOM snapshot / script click on a real site in Safari; whether `navigator.webdriver` is false.
- AX fallback: finding Safari's `AXWebArea` + `AXURL` binding to the target tab; real `CGEvent` typing.
- Per-profile enablement and pairing (spike 11), handler → daemon socket from the app sandbox (spike 7).

## To resume

Rebuild, launch the app once, then in Safari: Settings → Advanced "Show features for web developers";
Developer → "Allow unsigned extensions"; create a spare profile; enable "FERB Spike 5" there only with
"Always Allow on Every Website"; open a window in that profile; run
`uv run --python 3.12 --no-project --with pyobjc-framework-ApplicationServices --with pyobjc-framework-Quartz --with pyobjc-framework-Cocoa --with pytest pytest -q test_spike.py`.
