# Spike 4 — FERB extension in Brave and Chrome (native messaging, background tab, trusted input)

Status: done (2026-09-28). 16/16 tests pass, 5 runs in a row, in Chrome 154.0.8037.57 and Brave 151.1.93.136.

Spare profiles only: Brave gets a fresh temporary profile per run (`--load-extension`); Chrome uses a persistent
spare profile in `.spare-profiles/` (git-ignored) where the owner loaded the extension by hand. No remote
debugging flags anywhere: all control is extension → native host → Unix socket (stand-in daemon).

Run: `uv run --python 3.12 --no-project --with pytest pytest -q test_spike.py`
(Chrome: after any change to `ext/`, reload the extension in the spare profile and bump `version`.)

## Proven

- **Native messaging round trip** in both browsers; `hello` carries extension id and version.
- **Background tab** (`active: false`) opened, snapshotted (`visibilityState: hidden`), closed. Snapshot redacts
  password fields. `navigator.webdriver` is **false** (unlike CDP over pipe/port, spike 3).
- **Trusted click + typing in a background tab** via `chrome.debugger`: every event `isTrusted`, value lands, the
  tab is never made active, and the page is `hidden` again after detach.
- **Bot-protected site**: Google Search (signed out) — typed query + Enter in a background tab reaches `/search`,
  not the `/sorry/` automated-traffic page, in both browsers, every run.
- **Extension-side CDP allowlist** refuses `Network.getAllCookies`, `Runtime.evaluate`, `Target.getTargets`,
  `Storage.getCookies`.

## What it took (each is a requirement for build step 6)

1. **Background input needs focus emulation.** A hidden tab drops all debugger-dispatched input.
   `Emulation.setFocusEmulationEnabled {enabled: true}` makes it accept input without activating the tab.
   **Side effect: while attached, the site sees `document.visibilityState === "visible"`** although the owner
   doesn't see the tab. Owner decision pending (see report).
2. **"visible" is not "ready".** For a moment after emulation, the tab still drops *everything* (click included).
   Readiness probe: a listener in the extension's isolated world (invisible to the page) records a harmless
   debugger-dispatched `mouseMoved`; real input starts only after it lands. Without it ~50% of Brave attempts
   lost all input; with it 10/10 and 5 full runs green.
3. **MV3 service worker lifecycle**: the worker starts only for events it listens to. Register
   `chrome.runtime.onStartup` from the very first installed version, or a browser restart never connects.
   Chrome keeps running a cached unpacked worker across restarts (even after a `version` bump) until the
   extension is reloaded by hand → the daemon must compare `hello.version` with the expected protocol version and
   refuse a mismatch (the harness does).
4. **`chrome.scripting.executeScript` returns `result: null` when the injected function throws** (e.g. Google has
   elements whose `value` isn't a string). Treat null/`error` as a failure; coerce values in the snapshot.
5. **Native host must live outside TCC-protected folders** (`~/Desktop`, `~/Documents`, `~/Downloads`): the
   browser launches it but can't execute it; the only symptom is "Native host has exited".
6. **Loading unpacked**: branded Chrome 154 ignores `--load-extension` (also with
   `--disable-features=DisableLoadExtensionCommandLineSwitch`); Brave 151 accepts it.
7. **Native-host manifest lookup with a non-default `--user-data-dir`** (measured, one location at a time):
   Chrome 154 → `<user-data-dir>/NativeMessagingHosts`; Brave 151 → **Google Chrome's** default folder, not its
   own and not `<user-data-dir>`. Behaviour of the owner's *default* Brave profile is not tested here (spike 10).

## "Being debugged" bar (Chrome 154, checked 2026-09-28 with window-only screenshots)

- Attaching `chrome.debugger` to one **background** tab shows `"<extension name>" started debugging this browser`
  with a **Cancel** button in **every window of that browser profile**: the window holding the tab (above an
  unrelated active tab) and a second window with no involvement.
- It is not per-tab: the text says "this browser".
- After detach it **lingers ~5 s** (still full at 4 s, collapsing at 5 s, gone at 6 s).
- **Cancel** is documented to detach the debugger (not clicked in this check). If so, the owner can end a
  trusted-input action from the bar; the daemon must treat that detach as an interruption (action outcome
  `unknown` unless verified). To confirm in build step 6.
- Consequence for owner browsers: every trusted-input action makes the bar appear across all windows of that
  profile for its duration + ~5 s. Page mode (no debugger) shows nothing.

## Not verified

- Default-profile manifest locations in the owner's real Chrome/Brave (spike 10).
- Stale extension after an automatic update path (Web Store / External Extensions) — spike 10/13.
