> Current implementation note (0.19.0): AX is now the primary semantic view, with DOM-backed references and
> explicit fallback diagnostics. CDP frame identities replace URL matching; task-tab screenshot/point tools are
> registered. Sections describing the earlier DOM-first plan remain historical probe notes.
> See [0.19.0 recovery contracts](RELIABILITY_019.md) and ARCHITECTURE.md for the current implementation.

# PLATFORM_BROWSER.md — controlling a Chromium browser in the background

Engineering reference for the FERB browser driver rework (REWORK_PLAN.md phase 1). Scope: what an MV3 extension
plus a native host can do to tabs and windows **the owner isn't looking at**, without moving them. Chrome and
Brave in detail; Safari and Firefox as honest notes at the end.

Every claim is tagged:

- **[doc]** — Chromium / Chrome extension / CDP documentation or Chromium source, named inline.
- **[probe]** — measured on this Mac. Probes of 2026-10-02 are in section 12 verbatim; earlier spikes are cited as
  `spike N` (their FINDINGS.md files under `spikes/`).
- **[belief]** — unverified working knowledge. Treat as a hypothesis to probe before the driver relies on it.

Per mechanism, three questions are answered: **bg tab?** (works on a tab that isn't the active one in its window),
**minimized?** (works while the tab's window is minimized), **visible?** (what the owner can see of it).

Environment of the probes: macOS Darwin 25.6 (macOS 26), Google Chrome 154.0.8037.97, a throwaway
`--user-data-dir` under the session scratchpad (never a real profile), frontmost app polled every 50 ms with
`NSWorkspace.frontmostApplication`. Why not the "FERB Test" profile (Profile 25): it lives inside the owner's
running Chrome instance (pid 24974, launched for Profile 22 = FERB's identity), which can't be reached by CDP
without pairing Profile 25 into the live daemon (`ext_installs` holds only Profile 22), and a second Chrome on the
same data dir hands its command line to the running one. A fresh data dir is the only safe standalone option.

---

## 1. The MV3 extension model

### 1.1 Service worker lifetime

- The background script is a service worker: no DOM, no persistent globals, event-driven start. It is stopped after
  **30 s without activity**; every extension API call or delivered event resets the timer; a single request (e.g.
  `fetch`) is capped at **5 min** [doc: developer.chrome.com, "Extension service worker lifecycle"]. Since Chrome 110
  active workers are no longer hard-killed at 5 min [doc, same page].
- `chrome.alarms` minimum period is **30 s** (Chrome 120+) [doc: alarms reference]. Spike 13 measured the reconnect on
  the alarm at 29.3 s and on any `tabs.onUpdated` at ~0.1 s [probe: spike 13].
- Killing the worker drops the native-messaging port **immediately** (~10 ms) [probe: spike 13]. Whether an *idle*
  open native port keeps the worker alive is [belief: no — traffic on the port resets the idle timer, silence doesn't].
  The safe design: the daemon pings every ≤20 s while a task is running; the extension answers (an API call,
  resets the timer).
- State that must survive a worker restart goes in `chrome.storage.session` (survives worker restarts, not browser
  restarts) [doc; probe: spike 13 used it for seen ids / owned tabs].
- Events registered **synchronously at top level** are the only ones that wake a stopped worker; `runtime.onStartup`
  must exist from the first installed version or a browser restart never reconnects [doc; probe: spike 4].
- Unpacked extensions: branded Chrome 137+ ignores `--load-extension` [doc: Chrome 137 release notes; probe: spike 4];
  a lab Chrome with `--enable-unsafe-extension-debugging` + CDP `Extensions.loadUnpacked` loads one and keeps the
  pinned id [probe: spike 13, and probes E0/F0 below].
- bg tab / minimized: n/a (the worker has no window). visible: nothing.

### 1.2 Isolated worlds and content scripts

- A content script runs in an **isolated world**: same DOM, separate JS heap; the page can't see its globals and it
  can't see the page's [doc: "Content scripts" guide]. `chrome.scripting.executeScript({world: "MAIN"})` runs in the
  page's world when page JS state is needed (the probes read `window.__state()` that way) [doc; probe F2].
- Injection timing: static `content_scripts` with `run_at` `document_start` / `document_end` / `document_idle`
  (default); nothing is injected into pages loaded before the extension was (re)loaded — FERB injects on demand with
  `executeScript({files})` and an idempotency guard (`if (window.__ferb) return`), which is the right pattern [doc].
- `executeScript` targets `{tabId, frameIds | allFrames}`; results come back per frame with `frameId`; a throwing
  function yields `result: null` rather than an error [probe: spike 4].
- Events dispatched from any world with `dispatchEvent` have `isTrusted === false` [doc: UI Events]. `el.click()` fires
  an untrusted `click` only (no pointer/mouse sequence) [doc: HTML `click()`].
- Closed shadow roots are opaque to both worlds; open ones are traversable via `.shadowRoot` [doc: DOM].
- bg tab / minimized: **yes, both** — injection and DOM access don't depend on visibility [probe: spike 4 snapshotted a
  `hidden` tab; probe F2 read state from a background tab of a visible window; probe S4 from a minimized window via CDP].
  visible: nothing.

### 1.3 Tabs, windows, tab groups (`chrome.tabs`, `chrome.windows`, `chrome.tabGroups`)

- `chrome.tabs.create({windowId, url, active: false})`: "`active` — whether the tab should become the active tab in
  the window. Does not affect whether the window is focused" [doc: tabs reference].
  **[probe E3]** into a *minimized* window: tab created `active: false`, window stayed `minimized`, `focused: false`,
  no frontmost change. **[probe E4]** `active: true` into a minimized window: window **stayed minimized**, no
  frontmost change (the tab became that window's active tab without un-minimizing).
- `chrome.windows.create({url, focused, state, left, top, width, height})`. `focused` defaults to true; `false`
  "opens an inactive window" — Chromium calls `BrowserWindow::ShowInactive()` instead of `Show()`
  [doc: `chrome/browser/extensions/api/tabs/tabs_api.cc`, `WindowsCreateFunction`].
  **[probe E1, F0b]** `focused: false` → window reports `state: normal, focused: false` but **is not on screen**
  (`CGWindowListCopyWindowInfo` lists it with `kCGWindowIsOnscreen: false`); no frontmost change.
  **[probe E5]** `state: "minimized"` → not on screen, no frontmost change (the create() return value still said
  `normal`; `windows.get` afterwards is the truth).
  **[probe E7, F1]** default `focused: true` in this throwaway Chrome: window on screen, **Chrome did not become
  frontmost**, `windows.get` says `focused: true`.
  **Conflict:** in the owner's running Chrome a new window *did* pull Chrome to the front (owner's report 2026-10-01;
  spike 19: every method that created a window took focus, however launched). Reading [belief]: at *launch* Chrome's
  startup path activates the app for its first window, and macOS 14+ cooperative activation
  (`NSApplication.activate`, `yieldActivationToApplication`) decides whether a later `Show()` may activate — a Chrome
  the owner launched themselves may hold that right, a Chrome launched from a terminal tree does not. Not resolved
  here; the design must not depend on it (section 13).
- `chrome.windows.update(id, {state: "minimized"})` works and takes the window off screen [probe E2].
  `update(id, {state: "normal", focused: false})` **left the window minimized** [probe E8] — restoring without
  focusing isn't available through this API on macOS [probe]. Off-screen placement is refused: "Bounds must be at
  least 50% within visible screen space" [probe E6, error text verbatim].
- Window activation ≠ app activation: `focused: true` on a window activates the window inside Chrome; whether macOS
  brings Chrome forward is the question above. CDP's `Target.createTarget({newWindow: true, background: true})` and
  `Browser.setWindowBounds` never moved the frontmost app in any of 13 steps [probe S0–S13].
- A minimized window keeps its bounds (1200×799 here), so pages keep laying out at that viewport [probe S4 `inner`].
  To be independent of the window, use `Emulation.setDeviceMetricsOverride` (section 2.8).
- Tab groups: `chrome.tabs.group({tabIds, createProperties: {windowId}})` + `chrome.tabGroups.update(groupId,
  {title, color, collapsed})` [doc]. Grouping doesn't activate anything [probe: spike 4, the current extension groups
  every tab it opens]. visible: the group chip in that window's tab strip — nothing if the window is minimized.
- Discard / freeze: Memory Saver discards inactive tabs after a while (content gone until reload); opt a tab out with
  `chrome.tabs.update(tabId, {autoDiscardable: false})` [doc: tabs reference, `discarded`, `autoDiscardable`].
  Chrome also freezes long-hidden tabs (timers stop entirely; `freeze` lifecycle event) [doc: Page Lifecycle API];
  whether an attached debugger prevents freezing is [belief: yes, via the "being debugged" page lifecycle exemption].
- Minimized window, visible to the owner: a thumbnail at the right end of the Dock, and the window's entry in the
  Window menu [doc: macOS]. Nothing else.

### 1.4 Native messaging

- `chrome.runtime.connectNative(hostName)` starts the host as a child process; JSON messages over stdio with a
  4-byte native-endian length prefix; host→extension messages ≤ 1 MB, extension→host ≤ 4 GB; closing the port kills
  the host; the host must exit when stdin closes [doc: "Native messaging" guide].
- Host manifest lookup on macOS: `~/Library/Application Support/Google/Chrome/NativeMessagingHosts/<name>.json`
  (per user; here `com.ferb.native_host.json`) or `/Library/Google/Chrome/NativeMessagingHosts/` (all users) [doc];
  a custom `--user-data-dir` looks in `<user-data-dir>/NativeMessagingHosts` in Chrome 154, and Brave 151 looks in
  Google Chrome's folder [probe: spike 4]. The host binary must live outside TCC-protected folders (Desktop,
  Documents, Downloads) or it "exited" silently [probe: spike 4].
- `port.onDisconnect` must read `chrome.runtime.lastError` or Chrome logs an unchecked error [probe: spike 12].
- bg / minimized / visible: independent of windows; nothing visible.

---

## 2. CDP through `chrome.debugger`

### 2.1 Attach, detach, the infobar, Cancel

- `chrome.debugger.attach({tabId}, "1.3")`; `sendCommand(target, method, params)`; `detach`; `onEvent`; `onDetach(source,
  reason)` with `reason ∈ {"target_closed", "canceled_by_user"}` [doc: debugger reference]. Attach took **8 ms**
  [probe E9]. Only one extension at a time can be attached to a target; DevTools opened by the owner on that tab
  detaches the extension [doc].
- Restricted targets: `chrome://`, the Web Store, other extensions' pages and (by policy) anything blocked by
  `DeveloperToolsAvailability` cannot be attached [doc].
- Child sessions: since Chrome 125 `Debuggee` may carry a `sessionId` (a `DebuggerSession`), so `Target.setAutoAttach
  {autoAttach: true, flatten: true, waitForDebuggerOnStart: false}` lets the extension talk to out-of-process iframes
  and workers through the tab session [doc: debugger reference, "DebuggerSession"].
- **Infobar [probe F2, screenshots + AX tree]:** while attached, every window of that profile shows a bar
  `"<extension name>" started debugging this browser` with a **Cancel** button and a close (×) — AX tree:
  `AXGroup:Infobar Container > AXGroup:Infobar > AXStaticText:"FERB probe ext" started debugging this browser,
  AXButton:Cancel, AXButton:Close`. It showed on the window whose *active* tab was a different page (the attached tab was
  a background tab). It was still present in byte-identical window captures **1 s and 3 s after detach**; spike 4 saw
  it collapse at 5 s and gone at 6 s [probe: spike 4]. It is per profile, not per tab: a second attach while one is
  active adds no second bar [belief: one bar per browser; probe E10 ran with no on-screen window so it couldn't confirm].
  A **minimized** window shows no bar (nothing of it is visible), and FERB's identity profile has no owner windows — so a
  persistent attach there is invisible; in an *owner* profile the bar sits in their windows for as long as FERB is
  attached.
- **Cancel** detaches with `reason: "canceled_by_user"`; the × only hides the bar [doc: debugger reference]. Not
  clicked in any probe. After a forced detach: focus emulation ends (the page goes `hidden` again [probe S9]), pending
  `sendCommand`s reject, and the action in flight is of unknown outcome → ledger `unknown`, verify by evidence.
- Suppressing the bar: `--silent-debugger-extension-api` (Chromium switch) [doc: `chrome/common/chrome_switches.cc`];
  only usable on a Chrome FERB launches itself, never on the owner's.
- bg tab: yes. minimized: yes. visible: the bar, in every *visible* window of the profile.

### 2.2 `Emulation.setFocusEmulationEnabled`

- "Enables or disables simulating a focused and active page" [doc: CDP Emulation domain]. Playwright enables it on every
  page at init [doc: `packages/playwright-core/src/server/chromium/crPage.ts`].
- Effect measured [probe S6, S12, F2]: `document.visibilityState` **hidden → visible** (a `visibilitychange` fires),
  `document.hasFocus()` **true**, `requestAnimationFrame` **0/s → 60/s**, a 100 ms `setInterval` **1/s → 10/s**,
  IntersectionObserver callbacks start being delivered (the initial entry arrived only after enabling), a `loading=lazy`
  image loaded after scrolling. Disabling reverses it (`hidden`, rAF 0, timers 1/s) [probe S9]. The *active* tab of the
  same window keeps reporting `visible` [probe F2 `active_tab_state`].
- Mechanism [belief]: Chromium treats the emulated page as visibly captured, so the renderer leaves the hidden state —
  which is why rendering, screenshots and observers all come alive. The owner sees nothing; the *site* sees a visible,
  focused page while attached (spike 4 raised this; it is the price of background input).
- Input without it: in tabs created over raw CDP with `background: true`, a click landed with no emulation [probe S5,
  S12]; under `chrome.debugger` in a window Chrome considered focused, the press/release were **dropped** (only
  hover/move events arrived) at 0 s and again at 1 s after attach, and landed after enabling emulation [probe F2]; spike 4
  saw ~50 % drops in Brave without a readiness probe. Rule: always enable emulation right after attach, then send one
  `mouseMoved` and wait for a trusted `mousemove` in the isolated world before real input (cheap; keep it).
- bg tab: yes. minimized: yes. visible: nothing.

### 2.3 `Input.dispatchMouseEvent`

- Params: `type` (mousePressed / mouseReleased / mouseMoved / mouseWheel), `x`, `y` in **CSS px of the top-level
  viewport**, `modifiers` bitmask (Alt=1, Ctrl=2, Meta/Cmd=4, Shift=8), `button` (none/left/middle/right/back/forward),
  `buttons`, `clickCount`, `deltaX/deltaY` (wheel), `pointerType` (mouse/pen) [doc: CDP Input domain].
- Injected at the browser's `RenderWidgetHost` (`RenderWidgetHostImpl::ForwardMouseEvent`), so every resulting DOM event is
  **`isTrusted: true`**, and it is routed to the frame under the point — including OOPIFs — by
  `RenderWidgetHostInputEventRouter` [doc: `content/browser/renderer_host/render_widget_host_input_event_router.cc`].
  Coordinates for an element in an iframe = iframe element's rect in the parent + element rect inside the frame.
- Order observed for move+press+release on a button [probe S5]: `pointerover, pointerenter, mouseover, mouseenter,
  pointermove, mousemove, pointerdown, mousedown, focus, pointerup, mouseup, click` — all trusted, `pointerType: "mouse"`.
  Later clicks on the same element: `pointermove, mousemove, pointerdown, mousedown, pointerup, mouseup, click`.
- `clickCount: 2` on the press gives a `dblclick`; `button: "right"` gives `contextmenu` [doc]. Drag: pressed → several
  moved with `buttons: 1` → released [doc].
- `mouseWheel` scrolls **whatever scroller is under the point**: the window [probe S8: 2300 → 2700 after `deltaY: 400`,
  `wheel` 1, `scroll` 1] or an `overflow: auto` container [probe S8: `boxTop: 300`, window unchanged].
- bg tab: yes [probe S12, F2]. minimized: yes [probe S5–S8]. visible: nothing.

### 2.4 `Input.dispatchKeyEvent` and `Input.insertText`

- `dispatchKeyEvent`: `type` rawKeyDown / keyDown / keyUp / char; `key`, `code`, `windowsVirtualKeyCode`,
  `nativeVirtualKeyCode`, `text` (the character a keyDown produces), `unmodifiedText`, `modifiers`, `autoRepeat`,
  `isKeypad`, `location`, and macOS **`commands`** (editing commands like `selectAll`, `moveToBeginningOfLine`,
  `deleteBackward`) [doc: CDP Input domain].
- `keyDown {key:"a", text:"a"}` + `keyUp` → `keydown, keypress, beforeinput, input, keyup`, all trusted, value `a`
  [probe S7]. `Enter` with `text:"\r"` → `keydown, keypress, beforeinput(insertLineBreak), keyup` [probe S7].
  `keyDown {key:"a", modifiers:4, commands:["selectAll"]}` selected the whole field (`selectionStart 0, End 3`)
  [probe S7] — mac shortcuts need `commands`, the modifier alone isn't interpreted by the renderer [belief for other
  shortcuts; probe for ⌘A].
- `insertText {text}` "emulates inserting text that doesn't come from a key press, for example an emoji keyboard or an
  IME" [doc]: `beforeinput(insertText), input` only — no key events — into the focused element, including
  `contenteditable` [probe S7: `inp` got `abc`, `ce` got `hello`]. Fast and trusted; a site that watches `keydown` per
  character (autocomplete, hotkeys) needs per-key `dispatchKeyEvent`.
- It types into **whatever has focus**: focus the target first (a trusted click on it, [probe S7]) and re-check
  `document.activeElement` in the isolated world before typing a secret (spike 3's focus-stealing hole).
- bg tab / minimized: yes (with focus emulation) [probe S7]. visible: nothing.

### 2.5 Frames and OOPIFs

- Same-process iframes: mouse coordinates in the top viewport reach them; keys go to the focused frame. Cross-origin
  iframes are separate targets: attach them through `Target.setAutoAttach` on the tab session and send `Runtime`/`DOM`
  commands with that child `sessionId`; **input still goes through the top-level session** (the router delivers it)
  [doc: CDP Target domain; `DebuggerSession`]. Spike 3 attached OOPIFs over the pipe with `flatten: true` [probe].
- `chrome.scripting.executeScript({frameIds})` reaches every frame regardless of process [doc]; the current snapshot
  already returns per-frame results.
- bg / minimized: same as the tab. visible: nothing.

### 2.6 JavaScript dialogs

- `Page.enable` → event `Page.javascriptDialogOpening {url, message, type: alert|confirm|prompt|beforeunload,
  hasBrowserHandler, defaultPrompt}`; answer with `Page.handleJavaScriptDialog {accept, promptText}` [doc: CDP Page].
- While a dialog is open the page's JS is blocked, every other command on that tab stalls, and in a background tab the
  owner can't see or dismiss it [belief: Chrome's tab-modal dialog shows only when the tab is active]. So the attached
  extension must own dialogs: forward the event to the daemon, default `accept: false` (dismiss), accept only when the
  action expects it (e.g. a confirm after a consequential click, declared in the act call).
- bg / minimized: handled only through CDP. visible: nothing for a background tab.

### 2.7 Screenshots

- `Page.captureScreenshot {format, quality, clip, fromSurface, captureBeyondViewport, optimizeForSpeed}` [doc].
- Produced a full render of the page in a **minimized** window, with focus emulation on and off (26–27 KB PNGs, content
  verified by eye: button, typed values, scroll container) and of a **background tab** in a visible window [probe S9,
  S12, E9]. Older Puppeteer activated the target first because hidden tabs failed to capture; in Chrome 154 that was not
  needed [probe]. Keep `fromSurface: true` (default) [belief: the surface exists because the debugger keeps the
  compositor alive].
- bg tab: yes. minimized: yes. visible: nothing.

### 2.8 `Emulation.setDeviceMetricsOverride`, `DOM.setFileInputFiles`

- `setDeviceMetricsOverride {width, height, deviceScaleFactor, mobile}` fixes the layout viewport regardless of the
  window; `clearDeviceMetricsOverride` undoes it [doc]. Use it so FERB's minimized window size never dictates layout and
  so screenshots are reproducible [belief: harmless on a tab the owner never views].
- `DOM.setFileInputFiles {files, nodeId | backendNodeId | objectId}` sets a file input without any dialog [doc]; the
  files must be readable by the browser process. The node id comes from `DOM.getDocument` + `DOM.querySelector` on a
  FERB-stamped attribute, or from the AX node's `backendDOMNodeId` [doc]. bg / minimized: yes [belief: node-level, no
  rendering involved]. visible: nothing.

### 2.9 Reading the page over CDP

- `Accessibility.getFullAXTree {depth, frameId}` → nodes with `role`, `name`, `value`, `properties` (focusable,
  hidden, …), `backendDOMNodeId`, `childIds`; per frame, so OOPIFs need their own session [doc].
  `Accessibility.queryAXTree {accessibleName, role}` finds by name/role [doc]. `DOM.getBoxModel` /
  `DOM.getContentQuads` give rects for a backend node [doc].
- The AX tree is computed by Blink whether or not the tab is visible [belief; spike 4's allowlist includes it, not
  measured]. Cost: a Gmail-sized page is thousands of nodes; depth-limit or query.

---

## 3. Background-tab behaviour (no emulation)

Measured with the page hidden (minimized window or non-active tab), debugger attached, emulation off [probe S4, S9, S12]:

| Facility | hidden tab | with focus emulation | source |
|---|---|---|---|
| `document.visibilityState` | `hidden` | `visible` | [probe] |
| `document.hasFocus()` | false (true once an element was focused by input) | true | [probe] |
| `requestAnimationFrame` | **0 /s** (paused) | 60 /s | [probe]; HTML "update the rendering" skips hidden docs [doc] |
| `setInterval(100 ms)` | **1 /s** (aligned to 1 s) | 10 /s | [probe]; Chrome timer throttling [doc: "Timer throttling in Chrome 88"] |
| intensive throttling | after 5 min hidden, chained timers (nesting ≥ 5) run once per **minute** | not triggered (page is "visible") | [doc: same post] |
| IntersectionObserver | no callbacks (initial entry arrived only once emulation was on) | delivered | [probe] |
| `loading=lazy` images | not loaded below the fold | loaded after scrolling | [probe] (hidden case [belief], follows from IO) |
| layout / `getBoundingClientRect` | **works**, rects correct (`btn [100,300,200,50]`) | works | [probe S4] |
| `scrollTo` / `scrollIntoView` | works [belief: not isolated from emulation in the probe] | works [probe S8] | |
| media autoplay | muted autoplay allowed; with sound needs user activation / MEI; a background tab can start audible media only after a trusted click (a CDP click counts) | same | [doc: Chrome autoplay policy] [belief on the CDP-click part] |
| `Page.captureScreenshot` | works | works | [probe S9] |
| freeze (Page Lifecycle) | hidden ≥ 5 min and idle → frozen, timers stop | not frozen | [doc] [belief that the debugger exempts it] |

Consequence: with the debugger attached and emulation on, a background tab behaves like a visible one for the site —
SPA routers, infinite scroll, lazy images and polling all run. Without emulation, pages that wait on rAF or IO (most
React apps for lists, all virtual scrollers) stall.

---

## 4. Trusted vs untrusted events

- `event.isTrusted` is true only for UA-generated events [doc: UI Events]. CDP input is UA-generated [probe: every event in
  S5–S7 trusted]; `dispatchEvent`, `el.click()`, `execCommand` from a script are not (except that `execCommand`'s
  resulting `input` event *is* trusted — it is a UA edit) [doc: MDN `execCommand`].
- What sites check [belief, from field experience]: `isTrusted` on click/submit (some banks, ticketing), the full
  pointer sequence and timing (bot scoring like reCAPTCHA v3), `keydown`→`input` consistency (payment iframes), user
  activation for popups/clipboard/fullscreen/audible autoplay (transient activation is granted by trusted
  keydown/mousedown/pointerup, not by synthetic events) [doc: HTML user activation].
- React and friends: React attaches one native listener per event type at the root and builds synthetic events from
  native ones; for inputs it compares the DOM value against its tracked value, so a programmatic `value=` must go
  through the **prototype setter** then fire `input` (the current `setValue` does this) [doc: React `inputValueTracking`].
  Trusted `insertText` needs no trick [probe S7].
- `contenteditable` editors (Gmail compose, Google Docs, ProseMirror, Lexical, Slate, Quill, Draft) listen to
  `beforeinput` / `input` with `inputType`, composition events, and selection; setting `textContent` bypasses their
  model. Use `Input.insertText` (trusted `beforeinput insertText` + `input`, [probe S7]) or, without the debugger,
  `document.execCommand("insertText", false, text)` — deprecated but implemented, fires `beforeinput`/`input` with
  `inputType: insertText`, undoable [doc: MDN]. Line breaks: `Input.dispatchKeyEvent Enter` (`insertLineBreak` /
  `insertParagraph` per editor) [probe S7 for the single-line case].
- Untrusted page-mode actions remain useful for plain forms, `<select>` (`value` + `change`; CDP can't open a native
  popup in a hidden tab [belief]) and for reading. Rule from the probes: anything consequential (send, submit, pay)
  goes through trusted input.

---

## 5. Scrolling

- Three scrollers: the window (`document.scrollingElement`), overflow containers (chat lists, side panels, modals), and
  nested frames. A snapshot that only reports `window.scrollY` misses the container the model is "scrolling".
- `scrollIntoView({block: "center"})` on an element scrolls every ancestor scroller as needed [doc: CSSOM View]; then
  re-read the rect [probe S8: `far` moved to y 331].
- `Input.dispatchMouseEvent mouseWheel` over a point scrolls the innermost scroller there, trusted, which is what
  virtualised lists (react-window, Gmail's thread list) react to [probe S8 for window and container; virtual-list claim
  [belief]].
- Virtual lists render only rows near the viewport: `find`-by-text over `innerText` misses off-screen rows; scroll,
  re-snapshot, repeat; detect `at_end` by `scrollTop + clientHeight >= scrollHeight - 2` on the *container* [belief].
- bg / minimized: yes [probe S8 in a minimized window]. visible: nothing.

---

## 6. Snapshots — what the model needs to act

Goal: a list of actionable elements with stable ids, plus enough geometry to click and to know what is on screen.

- **DOM walk in the isolated world (current approach)**: cheap, no debugger, rects per frame, labels from
  `aria-label`/`labels`/`placeholder`/`innerText`, values with secrets masked. Missing today: an **in-view flag** (rect
  intersects the viewport *and* `elementFromPoint(centre)` is the element or its descendant → not occluded), the
  **scroll position** of the window and of each element's nearest scroll container, **shadow DOM** (walk open
  `shadowRoot`s), **frame offset** so iframe elements get top-viewport coordinates (the parent frame reports the iframe
  element's rect; the daemon adds), and **container ids** so `scroll` can target one.
- **AX tree via `Accessibility.getFullAXTree`**: better names for icon-only buttons and ARIA widgets, roles the DOM walk
  can't infer, `backendDOMNodeId` for `DOM.*` follow-ups; no rects (one `DOM.getBoxModel` per node) and per-frame
  sessions. Best as a fallback when the DOM walk finds few interactive elements (canvas / ARIA-heavy apps) [belief].
- **Canvas** (Docs, Sheets, Figma, maps): no DOM to read; screenshot + AX (Google Docs exposes an AX layer) [belief].
- Visibility semantics for the model: `in_view` (clickable now), `offscreen_below/above` (scroll first), `hidden`
  (omit). Rects in top-viewport CSS px so a click is `Input.dispatchMouseEvent` at the centre with no translation.
- Pages change between snapshot and act: keep generation-scoped ids (already done) and re-check `isConnected` [done].

---

## 7. Profiles and launching without activation

- Profiles are folders under the data dir; `Local State → profile.info_cache` maps folder → name/email [doc; FERB's
  `profiles.py`]. `--profile-directory=<folder>` selects one; an unknown folder silently creates a profile [doc].
- `--no-startup-window` loads the profile (and its extensions) with no window [doc: chromium switches]; `open -g -b
  <bundle> --args …` launches without bringing the app forward [doc: `man open`]. Spike 19: these never moved the owner;
  arguments reach a *running* Chrome only through its binary (the singleton forwards them), `open` passes them only at
  launch [probe: spike 19; current ARCHITECTURE §7].
- Window activation on new window: see 1.3 — never observed from a terminal-launched Chrome [probe], observed in the
  owner's Chrome [probe: owner 2026-10-01]. Keep the daemon's frontmost tripwire + put-back.
- Any `--remote-debugging-*` flag sets `navigator.webdriver = true` and Google refuses new sign-ins [probe: spike 3];
  the extension path leaves it `false` [probe: spike 4]. Hence extension + `chrome.debugger`, not a debugging port, for
  owner-facing sites.
- bg / minimized / visible: a windowless load is invisible; the Dock shows Chrome running.

---

## 8. Safari (honest notes)

- No CDP and no `debugger` API for extensions; Safari Web Extensions get `browser.tabs/scripting/runtime/storage` and
  native messaging **only through a containing macOS app** (FERB.app) [doc: Apple "Safari web extensions"]. Safari App
  Extensions (older, Swift-only) are deprecated in favour of web extensions [doc].
- `tabs.create({active: false})` works; trusted input, screenshots of background tabs and dialog handling do **not**
  exist for extensions [doc]. Content-script `dispatchEvent` is untrusted, same as Chrome.
- AppleScript `tell application "Safari" to do JavaScript "…" in document 1` requires **Develop ▸ Allow JavaScript
  from Apple Events** (off by default; the Develop menu itself must be enabled) [doc: Apple]. It runs in the page's
  world, untrusted, visible: nothing, but it works on background tabs/windows [belief].
- `safaridriver` (WebDriver) drives a separate automation session with a visible "automation" glass pane; not a
  background mechanism [doc: Apple "Testing with WebDriver in Safari"]. Web Inspector's remote protocol is for
  Apple tooling, not scriptable by third parties [belief].
- Verdict: reading and navigation possible; trusted background input not. Mark Safari "read/navigate only" in the
  capability profile.

## 9. Firefox (honest notes)

- No `chrome.debugger`; `browser.tabs.create({active: false})`, `scripting.executeScript`, native messaging exist
  [doc: MDN WebExtensions]. Trusted input from an extension: none.
- Remote control is **WebDriver BiDi** (and the older Marionette / CDP shim, CDP removed in Firefox 129) over
  `--remote-debugging-port`; `input.performActions` gives trusted input, `browsingContext.captureScreenshot` works
  on background contexts [doc: Firefox remote protocol docs]. It flags the session as automated (`navigator.webdriver`)
  like Chrome's port [belief]. No per-tab infobar; a robot icon in the URL bar [doc].
- Verdict: a separate driver (BiDi client) if ever needed; not the extension.

---

## 10. Summary matrix

| Mechanism | bg tab | minimized window | visible to owner |
|---|---|---|---|
| content script read / untrusted act | yes [probe] | yes [probe] | nothing |
| `tabs.create active:false` | yes [probe E3] | yes, stays minimized [probe E3] | nothing (tab strip only if window shown) |
| `windows.create state:minimized` | — | yes [probe E5] | Dock thumbnail |
| `chrome.debugger` attach | yes [probe] | yes [probe] | infobar in every shown window of the profile [probe F2] |
| focus emulation | yes [probe] | yes [probe] | nothing (site sees "visible") |
| mouse / key / insertText | yes [probe F2, S12] | yes [probe S5–S7] | nothing |
| wheel scroll (window / container) | yes [belief] | yes [probe S8] | nothing |
| screenshot | yes [probe S12] | yes [probe S9] | nothing |
| JS dialog handling | CDP only [doc] | CDP only [doc] | nothing (bg) |
| file upload (`DOM.setFileInputFiles`) | yes [belief] | yes [belief] | nothing |
| AX tree | yes [belief] | yes [belief] | nothing |
| Safari extension | read/navigate | read/navigate | nothing |
| Firefox extension | read/navigate | read/navigate | nothing |

---

## 11. Known gaps to probe before relying on them

1. Whether `chrome.windows.create` activates Chrome **in the owner's own running Chrome** (conflicting evidence, 1.3).
2. Infobar end-of-linger (spike 4: 5–6 s) and one-bar-vs-two under concurrent attaches (E10 had no visible window).
3. Dialog delivery in a background tab (`Page.javascriptDialogOpening` without the tab ever becoming active).
4. `DOM.setFileInputFiles` through `chrome.debugger` on a hidden tab.
5. Does an attached debugger prevent freeze/discard of a tab hidden > 5 min.
6. The no-emulation input drop: reproduce the exact condition (window `focused: true` in Chrome vs not).

---

## 12. Probe results on this Mac (2026-10-02)

Chrome 154.0.8037.97, throwaway `--user-data-dir` in the scratchpad, `--remote-debugging-pipe --no-startup-window`;
`frontmost_changes` = every change of the frontmost app during the step (50 ms poll); `chrome_took_focus` = the probe
Chrome was ever frontmost; `on_screen` / `windows_on_screen` = `CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly)`
rows owned by the probe Chrome. Scratchpad paths shortened to `<scratch>`. Scripts: `probe.py`, `probe2.py`, `probe4.py`,
`page.html`, `ext2/` (kept in the session scratchpad, not in the repo).

### Probe 1 — raw CDP over the pipe (`probe.py`), owner app at start: com.brave.Browser

```
{"step": "S0 launch chrome --no-startup-window (no window)", "result": {"pid": 16354, "product": "Chrome/154.0.8037.97", "windows_on_screen": []}, "frontmost_changes": [[0.0, "com.brave.Browser"]], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "S1 Target.createTarget newWindow:true background:true (first window)", "result": {"targetId": "E1FAAD4D…", "windowId": 1599955534, "bounds": {"left": 22, "top": 55, "width": 1200, "height": 799, "windowState": "normal"}, "windows_on_screen": [["about:blank", {"X": 22.0, "Height": 799.0, "Y": 55.0, "Width": 1200.0}]]}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "S2 Browser.setWindowBounds windowState:minimized", "result": {"bounds": {"left": 22, "top": 55, "width": 1200, "height": 799, "windowState": "minimized"}, "windows_on_screen": []}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "S3 Target.createTarget newWindow:false background:true into the minimized window", "result": {"targetId": "CAEA72BF…", "windowId": 1599955534, "same_window_as_A": true, "bounds": {"left": 22, "top": 55, "width": 1200, "height": 799, "windowState": "minimized"}, "windows_on_screen": []}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "S4 attach to tab B (minimized window): state + rAF/timer rates, no focus emulation", "result": {"visibilityState": "hidden", "hidden": true, "hasFocus": false, "activeElement": "BODY", "scrollY": 0, "inner": [1200, 712], "btn": [100, 300, 200, 50], "inp": [100, 400, 308, 36], "ce": [100, 460, 304, 64], "far": [100, 2500, 200, 50], "box": [500, 300, 204, 204], "boxTop": 0, "raf": 1, "timer": 4, "clicks": 0, "trusted_clicks": 0, "inpValue": "", "ceText": "", "wheel": 0, "scrolls": 0, "boxscroll": 0, "io": [], "vis": [], "focus": [], "lazy_loaded": false, "lazy_complete": false, "webdriver": true, "rates_no_emulation": {"raf_per_s": 0.0, "timer100ms_per_s": 1.0}}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "S5 Input.dispatchMouseEvent click on tab B WITHOUT focus emulation", "result": {"click_at": [200.0, 325.0], "clicks": 1, "trusted_clicks": 1, "events": [["btn", "pointerover", true, "mouse", 11068], ["btn", "pointerenter", true, "mouse", 11068], ["btn", "mouseover", true, "", 11068], ["btn", "mouseenter", true, "", 11068], ["btn", "pointermove", true, "mouse", 11068], ["btn", "mousemove", true, "", 11068], ["btn", "pointerdown", true, "mouse", 11074], ["btn", "mousedown", true, "", 11075], ["btn", "focus", true, "", 11075], ["btn", "pointerup", true, "mouse", 11079], ["btn", "mouseup", true, "", 11079], ["btn", "click", true, "mouse", 11080]], "hasFocus": true, "visibilityState": "hidden"}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "S6 Emulation.setFocusEmulationEnabled true on tab B, then click", "result": {"after_enable": {"visibilityState": "visible", "hidden": false, "hasFocus": true, "activeElement": "btn", "vis": [["visible", 11994]], "focus": [["win-focus", 6052], ["win-focus", 6052]]}, "rates_with_emulation": {"raf_per_s": 60.0, "timer100ms_per_s": 10.0}, "click": {"clicks": 2, "trusted_clicks": 2, "events": [["btn", "pointermove", true, "mouse", 13519], ["btn", "mousemove", true, "", 13519], ["btn", "pointerdown", true, "mouse", 13520], ["btn", "mousedown", true, "", 13520], ["btn", "pointerup", true, "mouse", 13521], ["btn", "mouseup", true, "", 13521], ["btn", "click", true, "mouse", 13521]], "activeElement": "btn"}}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "S7 keys and insertText on tab B (focus emulation on)", "result": {"keyDown_a": [["keydown", true, "a", ""], ["keypress", true, "a", ""], ["beforeinput", true, "a", ""], ["input", true, "a", "a"], ["keyup", true, "a", "a"]], "insertText_bc": [["beforeinput", true, "bc", "a"], ["input", true, "bc", "abc"]], "enter": [["keydown", true, "Enter", "abc"], ["keypress", true, "Enter", "abc"], ["beforeinput", true, "insertLineBreak", "abc"], ["keyup", true, "Enter", "abc"]], "cmd_a_selectAll": {"keys": [["keydown", true, "a", "abc"], ["keyup", true, "a", "abc"]], "selection": [0, 3, "abc"]}, "contenteditable_insertText": [["beforeinput", true, "hello", ""], ["input", true, "hello", "hello"]], "state": {"inpValue": "abc", "ceText": "hello", "activeElement": "ce"}}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "S8 scrolling in tab B (minimized window, focus emulation on)", "result": {"after_scrollTo_2300": {"scrollY": 2300, "far": [100, 200, 200, 50], "btn": [100, -2000, 200, 50], "io": [[false, 12000], [true, 16454]], "scrolls": 1, "lazy_loaded": true, "lazy_complete": true}, "after_mouseWheel_400": {"scrollY": 2700, "wheel": 1, "scrolls": 2}, "container_wheel_300": {"scrollY": 0, "boxTop": 300, "boxscroll": 1}, "scrollIntoView_far": {"scrollY": 2169, "far": [100, 331, 200, 50], "io": [[false, 12000], [true, 16454], [false, 17070], [true, 18586]]}, "lazy_img_after_scroll_3400": {"lazy_loaded": true, "lazy_complete": true, "scrollY": 3288}}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "S9 Page.captureScreenshot of tab B minimized: emulation on, then off", "result": {"with_emulation": {"file": "<scratch>/B_min_emul_on.png", "bytes": 26970}, "after_disable": {"visibilityState": "hidden", "hasFocus": true, "vis": [["visible", 11994], ["hidden", 21431]], "focus": [["win-focus", 6052], ["win-focus", 6052]]}, "rates_after_disable": {"raf_per_s": 0.0, "timer100ms_per_s": 1.0}, "without_emulation": {"file": "<scratch>/B_min_emul_off.png", "bytes": 26745}}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "S10 Target.createTarget newWindow:true windowState:minimized (second window, born minimized)", "result": {"targetId": "6D3A3CA1…", "windowId": 1599955537, "bounds": {"left": 44, "top": 77, "width": 1200, "height": 799, "windowState": "minimized"}, "windows_on_screen": []}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "S11 Browser.setWindowBounds windowState:normal on window A (restore)", "result": {"bounds": {"left": 22, "top": 55, "width": 1200, "height": 799, "windowState": "normal"}, "windows_on_screen": []}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "S12 background tab C in the now-visible window A: input without/with focus emulation, screenshot", "result": {"targetId": "AF97833E…", "windowId": 1599955534, "state_no_emul": {"visibilityState": "hidden", "hasFocus": false}, "rates_no_emul": {"raf_per_s": 0.0, "timer100ms_per_s": 1.0}, "click_no_emul": {"clicks": 1, "trusted_clicks": 1, "n_events": 12}, "state_emul": {"visibilityState": "visible", "hasFocus": true}, "rates_emul": {"raf_per_s": 60.0, "timer100ms_per_s": 10.0}, "click_emul": {"clicks": 2, "trusted_clicks": 2, "n_events": 7, "first_events": [["btn", "pointermove", true, "mouse", 9467], ["btn", "mousemove", true, "", 9467], ["btn", "pointerdown", true, "mouse", 9468], ["btn", "mousedown", true, "", 9468], ["btn", "pointerup", true, "mouse", 9468], ["btn", "mouseup", true, "", 9468]]}, "screenshot": {"file": "<scratch>/C_bgtab_visible_window.png", "bytes": 24428}, "active_tab_still": [["CAEA72BF", "file:///…page.html"], ["AF97833E", "file:///…page.html"], ["6D3A3CA1", "about:blank"], ["E1FAAD4D", "about:blank"]]}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "S13 Browser.close", "result": {"close": {}}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
```

Screenshots B_min_emul_on / B_min_emul_off (minimized window) and C_bgtab_visible_window (background tab) all show the
rendered page (red button, `abc` in the input, `hello` in the contenteditable, the green scroll box) — not blank.
Note S11: after `windowState: normal` the window did not reappear in the on-screen list within 1 s (not investigated).

### Probe 2 — extension APIs from a loaded probe extension (`probe2.py`), owner app: com.brave.Browser

```
{"step": "E0 launch throwaway chrome + Extensions.loadUnpacked", "result": {"ext_id": "bincekjmeddbgjojgpmedblaimgkbcbb", "sw_targets": [["52BEA27A…", "chrome-extension://bincekjmeddbgjojgpmedblaimgkbcbb/sw.js"]], "windows_on_screen": []}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "E1 chrome.windows.create({url, focused:false})", "result": {"window": {"id": 1789418664, "focused": false, "state": "normal", "left": 22, "top": 55, "width": 1200, "height": 799}, "tab": 1789418665, "on_screen": []}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "E2 chrome.windows.update(w1, {state:'minimized'})", "result": {"state": "minimized", "on_screen": []}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "E3 chrome.tabs.create({windowId: minimized w1, active:false})", "result": {"tab": 1789418666, "tab_active": false, "window_state_after": "minimized", "window_focused": false, "on_screen": []}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "E4 chrome.tabs.create({windowId: minimized w1, active:true})", "result": {"tab": 1789418667, "window_state_after": "minimized", "window_focused": false, "on_screen": []}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "E5 chrome.windows.create({url, state:'minimized'}) (what FERB does today)", "result": {"window": {"id": 1789418668, "focused": false, "state": "normal"}, "on_screen": []}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "E6 chrome.windows.create off-screen (left/top 5000, focused:false)", "error": "Uncaught (in promise) Error: Invalid value for bounds. Bounds must be at least 50% within visible screen space.", "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "E7 chrome.windows.create({url}) with default focused (expected to take focus)", "result": {"window": {"id": 1789418670, "focused": false, "state": "normal"}, "on_screen": [["about:blank", {"X": 66.0, "Height": 799.0, "Y": 33.0, "Width": 1200.0}]]}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "E8 chrome.windows.update(w1, {state:'normal', focused:false}) (restore without focus)", "result": {"state": "minimized", "focused": false, "on_screen": [], "infobar_before": [{"window": 0, "title": "about:blank - Google Chrome", "texts": []}]}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "E9 chrome.debugger.attach on background tab T3: infobar per window, immediate click, detach linger", "result": {"tab": 1789418666, "attach_ms": 8, "immediate_click_no_emulation": {"clicks": 0, "trusted_clicks": 0, "n_events": 6, "visibilityState": "hidden", "hasFocus": true}, "infobar_during_attach": [AX scan depth 14 found no "debugging" text in the one listed window at t=5.4…7.6 s — the scan was too shallow, see probe 3], "after_emulation": {"visibilityState": "visible", "hasFocus": true}, "screenshot_b64_len": 32572, "detach_at": 8.7, "infobar_after_detach": [same shallow scan, nothing], "onDetach_events": []}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "E10 second attach while first still attached: one bar or two?", "result": {"one_attached": [...no text found (shallow scan)...], "two_attached": [...], "one_detached_other_still": [...]}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
{"step": "E11 Browser.close", "result": {"close": {}}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.brave.Browser"}
```

### Probe 3 — infobar by window capture + attach race (`probe4.py`), owner app: com.microsoft.VSCode

```
{"step": "F0 launch + load probe extension", "result": {"ext_id": "bincekjmeddbgjojgpmedblaimgkbcbb", "sw": "C935DB4E…"}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.microsoft.VSCode"}
{"step": "F0b chrome.windows.create({focused:false}): does the window exist on screen at all?", "result": {"win": 75260797, "get": {"state": "normal", "focused": false, "left": 22, "top": 55, "width": 1200, "height": 799}, "all_windows": [..., {"name": "about:blank", "onscreen": false, "layer": 0, "bounds": {"X": 22.0, "Height": 799.0, "Y": 55.0, "Width": 1200.0}}]}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.microsoft.VSCode"}
{"step": "F1 visible window (default focused) with an active tab and a background tab", "result": {"win": 75260801, "active_tab": 75260802, "bg_tab": 75260803, "get": {"state": "normal", "focused": true}, "all_windows": [..., {"name": "ferb probe", "onscreen": true, "layer": 0, "bounds": {"X": 22.0, "Height": 799.0, "Y": 55.0, "Width": 1200.0}}, ...], "shot": {"file": "<scratch>/F1_before_attach.png", "bytes": 127699}}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.microsoft.VSCode"}
{"step": "F2 attach to background tab: click at 0 s / 1 s / with emulation; window screenshots while attached and after detach", "result": {
  "click_at_0s": {"clicks": 0, "trusted_clicks": 0, "events": ["pointerover", "pointerenter", "mouseover", "mouseenter", "pointermove", "mousemove"], "visibilityState": "hidden", "hasFocus": true},
  "click_at_1s": {"clicks": 0, "trusted_clicks": 0, "events": ["pointermove", "mousemove"], "visibilityState": "hidden", "hasFocus": true},
  "shot_attached_1.5s": {"file": "<scratch>/F2_attached.png", "bytes": 141994},
  "ax_attached": [... "AXGroup:Infobar Container", "AXGroup:Infobar", "AXStaticText:\"FERB probe ext\" started debugging this browser", "AXButton:Cancel", "AXButton:Close" ...],
  "click_with_emulation": {"clicks": 1, "trusted_clicks": 1, "events": ["pointermove", "mousemove", "pointerdown", "mousedown", "focus", "pointerup", "mouseup", "click"], "visibilityState": "visible", "hasFocus": true},
  "active_tab_state": {"visibilityState": "visible", "hasFocus": true, "clicks": 0},
  "shot_attached_4s": {"file": "<scratch>/F2_attached_4s.png", "bytes": 141994},
  "detach_after_s": 13.5,
  "shots_after_detach": {"1s": {"bytes": 141994}, "3s": {"bytes": 141994}, "5s": {"error": "no on-screen window"}, "7s": {"error": "no on-screen window"}},
  "ax_after_7s": [] (no windows listed), "bg_tab_after_detach": {"visibilityState": "hidden", "hasFocus": true}}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.microsoft.VSCode"}
{"step": "F3 second attach (is the bar per attach?)", "result": {"reattach_shot": {"error": "no on-screen window"}}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.microsoft.VSCode"}
{"step": "F4 Browser.close", "result": {"close": {}}, "frontmost_changes": [], "chrome_took_focus": false, "frontmost_at_end": "com.microsoft.VSCode"}
```

F2_attached.png (window capture while attached to the *background* tab): Chrome window showing the *active* tab
`127.0.0.1:8767/page.html`, below the toolbar a dark bar `"FERB probe ext" started debugging this browser  [Cancel]  ×`.
F2_detached_1s.png and F2_detached_3s.png are byte-identical to it (bar still shown). At 5 s and 7 s the window was
no longer in the on-screen list and the AX tree listed no windows; cause not identified (not a focus change: the
poller saw none). The "~5 s linger" end stands on spike 4's measurement only.

Answers to the five probe questions:
1. **Click in a non-active tab / minimized window:** yes [S5–S7, S12, F2]; press/release were dropped right after a
   `chrome.debugger.attach` with no emulation [F2, E9] and landed once focus emulation was on.
2. **Focus emulation → `document.hasFocus()` true:** yes, and `visibilityState` becomes `visible`, rAF/timers unthrottled
   [S6, S12, F2].
3. **Infobar per attach, persisting while attached:** present while attached and ≥ 3 s after detach, in a window
   whose active tab was not the attached one [F2]; one-vs-two bars under concurrent attaches not established.
4. **Tab `active:false` in a minimized window, frontmost unchanged:** yes — no frontmost change in any of 29 steps,
   including window creation [E3, E5, S3, S10].
5. **Scrolling + `getBoundingClientRect` in a background tab:** yes [S4, S8], including container wheel-scroll and
   `scrollIntoView`; IntersectionObserver and lazy images need emulation on.

---

## 13. Ranked recommendation for the FERB extension redesign

Ranked by how much it buys relative to risk. Every item is background-first and leaves the owner's frontmost app alone.

1. **One FERB window per profile, born minimized, kept forever.** On the first `open` in a profile:
   `const w = await chrome.windows.create({url, state: "minimized", focused: false})` [probe E5, S10]; store `w.id`
   in `chrome.storage.session`; re-validate with `chrome.windows.get` (recreate if gone, re-minimize with
   `chrome.windows.update(id, {state: "minimized"})` if the owner restored it [probe E2]). Every later tab:
   `chrome.tabs.create({windowId, url, active: false})` [probe E3] + `chrome.tabs.update(tabId, {autoDiscardable: false})`
   + the existing `tabs.group` into "FERB". Never `windows.update({state: "normal"})`, never `focused: true`.
   Keep the daemon's frontmost tripwire and put-back (current ARCHITECTURE §7) because 1.3's conflict is unresolved; log a trail
   event when it fires so the conflict gets data.
2. **Persistent `chrome.debugger` attach per task tab, for the tab's life.** On `open`:
   `chrome.debugger.attach({tabId}, "1.3")`, then in order `Page.enable`, `Emulation.setFocusEmulationEnabled
   {enabled: true}`, `Target.setAutoAttach {autoAttach: true, flatten: true, waitForDebuggerOnStart: false}`,
   `Emulation.setDeviceMetricsOverride {width: 1280, height: 900, deviceScaleFactor: 1, mobile: false}`, one readiness
   `mouseMoved` + isolated-world `isTrusted` check. Detach on `close`. `onDetach` → mark the tab `detached`, reason to
   the daemon (`canceled_by_user` = the owner pressed Cancel: pause the task, outcome of the in-flight act `unknown`).
   Policy split: FERB's identity profile has no owner windows → the bar is invisible, attach freely. Owner profiles →
   the bar is in their windows for the task's duration; attach for the task (not per action, which flickers the bar
   every ~6 s) and tell the owner once per task ("FERB is working in Chrome “scaler”; a 'started debugging' bar shows
   until it's done"). Service-worker keepalive while attached: daemon ping every 20 s, answered with any API call.
3. **Snapshot = DOM walk + viewport semantics (+ AX fallback).** Extend `content.js`: `in_view` (rect ∩ viewport and
   `elementFromPoint` hit), `scroll: {y, height, viewport_h}` for the window and `container` id for each element's
   nearest scrollable ancestor (with its own `scroll` block), open-shadow-root traversal, iframe offset so rects are
   top-viewport (parent frame reports each `<iframe>` rect; the service worker composes by `frameId`/`parentFrameId`
   from `chrome.webNavigation.getAllFrames` or the frame tree), `occluded` flag. Allowlist `Accessibility.getFullAXTree
   {depth}` + `DOM.getBoxModel` for the fallback when < N interactive elements are found. Screenshot via
   `Page.captureScreenshot {format: "jpeg", quality: 70}` on request or for canvas pages.
4. **Scroll by trusted wheel, targeted.** `scroll {tab_id, container?, dy}` → `Input.dispatchMouseEvent {type:
   "mouseWheel", x, y, deltaX: 0, deltaY: dy}` at the container's (or viewport's) centre [probe S8]; before a click,
   `scrollIntoView({block: "center"})` in the isolated world then re-read the rect; return `{y, height, at_end}` for
   that scroller.
5. **Dialogs owned by the extension.** `Page.javascriptDialogOpening` → `send("dialog", {tab_id, type, message,
   default})` and auto-`Page.handleJavaScriptDialog {accept: false}` after a short grace unless the current act
   declared `expect_dialog: "accept"` (then `{accept: true, promptText}`); `beforeunload` accepted only on `close`.
6. **Keys and typing.** `press {key, modifiers}` → `Input.dispatchKeyEvent` with a full table (`key`, `code`,
   `windowsVirtualKeyCode`, `text` for printables, `modifiers` bitmask Alt 1 / Ctrl 2 / Meta 4 / Shift 8) and the mac
   `commands` for editing shortcuts (⌘A `selectAll`, ⌘← `moveToBeginningOfLine`, ⌫ `deleteBackward`) [probe S7].
   `type {text}` → trusted click on the element, `activeElement` check in the isolated world, then `Input.insertText`
   (fast, works in contenteditable) [probe S7]; `type {text, per_key: true}` → keyDown/keyUp per character for
   autocomplete boxes. Secrets: unchanged sealed `fill` (untrusted setter) — never `insertText`.
7. **Files and uploads.** `upload {el, paths}` → stamp the element with `data-ferb-node` in the isolated world,
   `DOM.getDocument {depth: 0}` + `DOM.querySelector` + `DOM.setFileInputFiles {files, nodeId}`; probe item 11.4 first.
8. **Keep page mode** for reads, `<select>`, plain forms; route anything with `effect ∈ {send, pay, destroy}` through
   trusted input, as the rework's action gate requires.

What this replaced in the pre-rework ARCHITECTURE.md §9.4 (`git show b993da5:ARCHITECTURE.md`; current §7
describes the implementation): "`chrome.debugger` attaches to one tab for those actions, then
detaches" becomes "attached for the task tab's life"; the allowlist gains `Page.enable`, `Page.handleJavaScriptDialog`,
`Target.setAutoAttach`, `Emulation.setDeviceMetricsOverride`, `DOM.getBoxModel`, `DOM.querySelector`,
`DOM.setFileInputFiles`, `Input.dispatchMouseEvent mouseWheel` (already allowed) — each with a parameter-shape check,
and `Target.*` stays refused except that one auto-attach call (owner approval per section 16).
