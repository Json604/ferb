# macOS platform reference: how apps are built and how FERB can drive them in the background

Written 2026-10-02 for rework phase 1 (`docs/REWORK_PLAN.md`). The driver (phase 3) must rely only on what this file
says works, and on nothing it marks unverified.

**Labels.** Every claim carries one of:

- `[doc]` — from Apple's headers or documentation (or the framework's own docs: Chromium, Electron), with the source
  named.
- `[probe]` — verified on this Mac by `ferb/act/mac/probe.py` or the side experiments recorded in section 13, with
  the result quoted. Earlier spikes are cited as `[probe, spike N]` (results in `PROGRESS.md` and the spike findings;
  the historical architecture notes are available with `git show b993da5:ARCHITECTURE.md`).
- `[belief]` — unverified. Treat as a hypothesis for the probe, never as a fact for the driver.

Machine: macOS 26.6.2 (25G83), Apple Silicon (1470×956 points main display), pyobjc 12.2.2 in `.venv`. Accessibility,
Screen Recording and Input Monitoring are granted to the terminal context the probe ran in (`AXIsProcessTrusted`,
`CGPreflightScreenCaptureAccess`, `CGPreflightListenEventAccess`, `CGPreflightPostEventAccess` all True `[probe]`).

Background means: the app is not the frontmost app, its window may be behind other windows or on another Space, and
the owner's focus, mouse pointer, keyboard and Space never change.

---

## 1. How Mac apps are built, and what each exposes to Accessibility

The Accessibility (AX) tree an assistive process sees is produced by the app's UI toolkit. The toolkit decides the
tree's shape, which elements have actions, and whether the tree exists at all until someone asks.

| Toolkit | Who | Tree shape | Actions on elements | Lazy? |
|---|---|---|---|---|
| AppKit (Cocoa) | TextEdit, Finder, Mail, Xcode, most Mac-native apps | Faithful: AXWindow > AXSplitGroup/AXScrollArea/AXGroup > controls with real roles (AXButton, AXTextArea, AXTable, AXOutline…). Menus under the app's AXMenuBar. `[doc: NSAccessibility protocol]` | Standard controls expose AXPress / AXIncrement / AXShowMenu etc. automatically. TextEdit's text area exposes only `AXShowMenu` but all of AXValue / AXSelectedText / AXSelectedTextRange / AXFocused are settable `[probe]`. | No. Tree is live from launch (TextEdit: 46 elements, Finder desktop window: 29 `[probe]`). |
| SwiftUI | Calculator (macOS 15+), System Settings panes, newer Apple apps | Derived from the view hierarchy: many AXGroup wrappers, controls appear as AXButton / AXStaticText / AXToolbar with `AXDescription` carrying the label. Calculator: 35 elements, buttons labelled by description ("1", "All Clear"), display is an AXStaticText `[probe]`. | Buttons expose AXPress and it works in the background `[probe]`. Custom tappable views (`onTapGesture` on a non-Button) expose no action and need a click `[belief]`. | No `[probe: Calculator]`. |
| Mac Catalyst (UIKit) | Some Apple apps (Messages pre-14, Maps, Podcasts), iPad-origin apps | UIKit accessibility bridged into NSAccessibility: flat lists of AXGroup / AXButton / AXStaticText with `AXDescription` labels, little hierarchy `[belief]`. | AXPress on buttons `[belief]`. | Unverified. |
| Electron (Chromium) | Slack, Discord, VS Code, WhatsApp Desktop (Electron build), Notion | Native shell (AXWindow) plus an AXWebArea whose children mirror the DOM accessibility tree: AXGroup, AXButton, AXTextField, AXStaticText, AXLink, with ARIA roles mapped `[doc: Chromium ax_platform_node_mac]`. | Buttons expose AXPress; text fields take AXValue and AXFocused `[probe, spike 18 and 20: WhatsApp]`. Many clickable DOM nodes are AXGroup with an AXPress action — hence the probe's `has_actions_on_groups` `[belief for general Electron; spike 18 saw pressable rows]`. | **Yes.** Chromium builds the full accessibility tree only once "accessibility mode" is on. Triggers: VoiceOver sets `AXEnhancedUserInterface` on the app element; Electron also honours a client setting `AXManualAccessibility` to true on the app element `[doc: electronjs.org/docs/latest/tutorial/accessibility]`; Chromium also switches on after seeing AX API traffic `[belief]`. The existing driver's `expose_tree` sets `AXManualAccessibility` once per pid. Non-Electron apps answer `-25205 kAXErrorAttributeUnsupported` `[probe: TextEdit, Calculator, Finder]`. |
| Chromium browsers | Chrome, Brave, Edge, Arc | As Electron: AXWindow > toolbar groups > AXWebArea per tab (only the visible tab's web area is populated) `[doc: Chromium]`. | AXPress on web buttons/links, AXValue on fields `[belief for the driver; FERB uses CDP for browsers instead]`. | Yes; the web area fills in after the first AX request. Existing driver re-reads for up to 2 s `[probe, earlier build steps]`. `AXEnhancedUserInterface` set on the app element is Chromium's VoiceOver signal and enables full accessibility `[doc: Chromium browser_accessibility_state_impl_mac.mm]`. |
| WebKit (Safari) | Safari, WebKit-based apps | AXWebArea with a DOM-derived tree, always on `[belief]`. | AXPress on web controls `[belief]`. | Tree is on from launch `[belief]`. |
| Qt | VirtualBox, OBS, Telegram Desktop, VLC (partly) | Qt's `QAccessible` bridge maps widgets to NSAccessibility roles; custom-painted widgets (QML without `Accessible` properties) are blank AXGroups `[doc: Qt QAccessible docs]`. | AXPress on QPushButton etc. `[belief]`. | Qt enables its bridge when the AX server first asks `[belief]`. |
| Java (Swing/JavaFX) | IntelliJ, DBeaver, Minecraft launcher | OpenJDK's macOS AX bridge (JavaNativeFoundation a11y) exposes Swing components as native roles; coverage depends on the JDK version `[belief]`. | AXPress for JButton `[belief]`. | Bridge is lazy: JDK enables it when the AX client asks `[belief]`. |
| Flutter (desktop) | Few macOS apps | The semantics tree is built only when assistive technology is active; FlutterView exposes AXGroup children with labels `[doc: Flutter docs say semantics is enabled on demand]`. | AXPress on semantics nodes with tap actions `[belief]`. | Yes, on demand `[doc]`. |
| Games / Metal / custom (SDL, Unity, Unreal) | Games, some creative tools | One AXWindow with no children, or nothing `[belief]`. | None. Mouse/keyboard events only. | n/a |
| Remote-view panels | Open/Save panels (`com.apple.appkit.xpc.openAndSavePanelService`), share sheets, Touch ID prompts | Hosted in a separate XPC process; appear in the host app's tree as an AXSheet / AXWindow whose contents are proxied `[doc: NSRemoteView is private; the openAndSavePanelService process is observable]`. | Buttons are pressable through the host app's tree once the sheet is listed (`Save`, `Cancel`, `Delete` under TextEdit's save sheet) `[probe]`. | The sheet appeared under the window's `AXChildren` (role AXSheet) while `AXSheets` was still empty `[probe]`. |

Two attributes on the **application element** matter for lazy trees:

- `AXEnhancedUserInterface` — set by VoiceOver when it starts; AppKit stores it and some apps (Chromium) treat it as
  "a screen reader is here". On this Mac it already read `True` on TextEdit and Calculator (because a trusted AX
  client — the probe — was connected), and setting it returned `-25208 kAXErrorNotImplemented` for every value type
  tried (Python `True`, `1`, `NSNumber`) `[probe]`. So: read it, don't rely on setting it; for AppKit apps it is
  already on when FERB talks to them. For Chromium it may be the needed trigger `[belief]`.
- `AXManualAccessibility` — Electron-specific `[doc]`; `-25205` elsewhere `[probe]`.

---

## 2. The Accessibility API surface for acting

All from `ApplicationServices/HIServices` (`AXUIElement.h`, `AXActionConstants.h`, `AXAttributeConstants.h`) `[doc]`.
Every call below needs the Accessibility TCC grant (`AXIsProcessTrusted`) `[doc]`; none activates the app
`[probe: front app never changed across ~60 AX calls in the three runs]`.

### 2.1 Actions (`AXUIElementPerformAction`)

| Action | What it does `[doc]` | Background? | Caveats |
|---|---|---|---|
| `AXPress` | The control's primary action (button click, menu item choose, tab select, row activate). | **Yes** `[probe: Calculator digits and All Clear; TextEdit and Calculator menu items; spike 18: WhatsApp tabs, New Chat; Electron buttons]`. | A press is a *semantic* click: no mouse position, no hover state, no modifiers. Returns `0` even when the app's handler does nothing visible (TextEdit's "Show Ruler" item returned 0 with no observable check to read) `[probe]`; verify by re-reading state. |
| `AXShowMenu` | Opens the element's contextual menu. | Yes `[belief]` | The menu then appears as an `AXMenu` under the app element; items pressable with AXPress `[belief]`. |
| `AXIncrement` / `AXDecrement` | Steppers, sliders, scroll bars. | Yes `[belief]` | Step size is the control's. |
| `AXConfirm` | "Return" semantics on a text field / combo. | Yes `[belief]` | |
| `AXCancel` | "Escape" semantics (close a panel). | Yes `[probe, spike 18: closed WhatsApp's New Chat panel]` | |
| `AXRaise` | Bring a window to the front *of its app* (does not activate the app). | Yes, but it reorders windows the owner may be looking at `[doc]`. | Used by the existing driver's `raise_window` only in the foreground path. |
| `AXPick` | Choose a menu item without closing (listed on TextEdit's menu items alongside AXPress and AXCancel) `[probe]`. | Yes `[belief]` | |
| `AXScrollToVisible` | Scroll the element into view inside its scroll area. | Yes `[belief]` | TextEdit's text area does **not** list it (only `AXShowMenu`) `[probe]`; scroll areas and table rows usually do `[belief]`. |
| `AXShowAlternateUI` / `AXShowDefaultUI` | Reveal hover-only UI. | Yes `[belief]` | |

### 2.2 Settable attributes (`AXUIElementSetAttributeValue`)

| Attribute | Use | Background? | Notes |
|---|---|---|---|
| `AXValue` | Replace a text field's / text area's whole value; toggle a checkbox; set a slider. | **Yes** `[probe: TextEdit text area; spike 18: WhatsApp search box]`. | Replaces everything; fires the app's change notifications (WhatsApp's search filtered `[spike 18]`). Not every field accepts it (`AXUIElementIsAttributeSettable` says) `[doc]`. |
| `AXSelectedTextRange` + `AXSelectedText` | **Editing**: set the caret or a selection, then set `AXSelectedText` to insert or replace exactly that range. | **Yes** `[probe: insert at end ("alpha " → "alpha bravo "), replace a middle range ("alpha" → "ALPHA"), both err 0 and value read back correct]`. | This is the precise, non-destructive edit primitive. Range units are UTF-16 code units `[doc]`. |
| `AXFocused` | Move the app's keyboard focus to an element without activating the app. | **Yes** `[probe: TextEdit text area became AXFocusedUIElement, CFEqual true]`. | Focus in a background app is the app-local "first responder"; keystrokes posted to the pid go there. |
| `AXVisibleCharacterRange` | Scroll a text area so a range is visible (settable on NSTextView) `[belief]`. | Yes `[belief]` | Untested; a candidate background scroll for text. |
| `AXMain` on a window | Make it the app's main window `[doc]`. | Yes `[belief]` | Existing driver sets it before AXRaise. |
| `AXMinimized` on a window | Minimise / restore `[doc]`. | Yes `[belief]` | |

### 2.3 Reading state the driver needs

| Attribute | Element | Background / other Space |
|---|---|---|
| `AXFocusedApplication` | system-wide | Fails intermittently with `-25204 kAXErrorCannotComplete` while the owner is in a full-screen app; `NSWorkspace.frontmostApplication` answered correctly each time `[probe: all runs recorded -25204 at least once]`. Use AX first, NSWorkspace as fallback (the probe's `front()` does). |
| `AXFocusedUIElement` | app | Works even when the window list is empty `[probe]`. |
| `AXWindows` | app | **Empty** for every app whose windows are on another Space (TextEdit, Calculator, Finder's only window was the desktop) `[probe; spike 18 saw the same for WhatsApp]`. |
| `AXFocusedWindow` / `AXMainWindow` | app | Work for other-Space windows `[probe; spike 18]`, but came back `None` in a TextEdit instance stuck in a modal quit sequence `[probe]`. Fall back to `AXFocusedUIElement` → `AXWindow`. |
| `AXSheets` | window | Was empty while a save sheet was up; the sheet was in the window's `AXChildren` with role `AXSheet` `[probe]`. Check both. |
| `AXMenuBar` / `AXExtrasMenuBar` | app | Menus readable and pressable in the background `[probe]`. |
| `AXMenuItemMarkChar` | menu item | Check mark; updates ~1 s after the press (0.8 s was too early for Calculator's View menu) `[probe]`. |
| `AXUIElementCopyElementAtPosition(app, x, y)` | app | Finds the element under a screen point **only when that window is on the current Space**: the owner's on-screen VS Code window → `AXGroup` of the right window; TextEdit/Calculator windows on another Space → the app's `AXMenuBar` (err 0, so check the role) `[probe]`. Occluded-on-the-same-Space case: not testable today (owner stayed in full-screen Spaces) — `[belief]` that it works, since the app element limits the search to that app's windows `[doc]`. |
| `AXUIElementCopyElementAtPosition(systemWide, x, y)` | system-wide | Returns the topmost element on the **current** Space (VS Code's) regardless of the target app `[probe]`. Only useful for "what would a real click hit". |

### 2.4 The Accessibility keyboard: `AXUIElementPostKeyboardEvent(app, char, keycode, down)`

Deprecated since 10.9 but present `[doc: AXUIElement.h]`. Posts a key to the app's focused element without the app
being frontmost `[doc]`. Results are **mixed**: WhatsApp's search box took it when `CGEventPostToPid` keys did not
`[probe, spike 20]`; TextEdit's focused text area and Calculator ignored it in every run today (`ax_post_key_ok:
false`) `[probe]`. Needs real virtual keycodes (US layout table), so no unicode typing. Keep as a per-app fallback,
learned by the profile, never the default.

---

## 3. Keyboard focus, key windows and the responder chain

- Each app has at most one **key window** (receives keyboard events) and one **main window** (the principal document
  window); the key window's **first responder** gets key events `[doc: NSWindow, NSResponder]`.
- An app that is not active still has a key window and a first responder of its own; AppKit keeps them when the app
  deactivates `[doc: NSApplication deactivate → key window resigns "key" status visually but `keyWindow` is restored on
  activation]`. Keys posted straight to the process go to that window's first responder `[probe: TextEdit took unicode
  and keycode events into its text area with VS Code frontmost]`.
- `AXFocused = true` on an element changes that first responder without activation `[probe]`.
- Sheets take key focus from their parent window; a modal panel (save/quit review) blocks menu actions until dismissed
  and can hold windows alive: TextEdit instance 16283, asked to `quit` with unsaved untitled documents, showed a Save
  sheet; `close every document saving no` emptied its document list but the window-server windows stayed
  ("Untitled", "Untitled 2", "Untitled 3", "Save") and later `File > New` windows piled up as zombies; `AXWindows`,
  `AXFocusedWindow`, `AXMainWindow` all went empty `[probe]`. Lesson: **never `quit` an app that may have unsaved
  documents in the background; close the documents you made, leave the app running.**

---

## 4. The event system

### 4.1 Where events can be injected `[doc: CGEvent.h]`

| API | Where the event enters | Who sees it | Background? | Occluded / other Space |
|---|---|---|---|---|
| `CGEventPost(kCGHIDEventTap, ev)` | Below the window server, as if from hardware. | The window server routes it like real input: mouse to the window under the pointer **on the current Space**, keys to the frontmost app's key window. | **No** for targeting: it goes wherever the owner's pointer / focus is. Moves the real pointer. | Can't reach either. Used by the existing foreground path only. |
| `CGEventPost(kCGSessionEventTap / kCGAnnotatedSessionEventTap)` | Session level (after HID taps). | Same routing as above `[doc]`. | No. | No. |
| `CGEventPostToPid(pid, ev)` | Straight into that process's event queue (10.11+). | Only that app. The window server does not route it, so the owner's pointer, focus and Space are untouched `[probe: mouse position and front app unchanged in every run]`. | **Keys: yes** — unicode (`CGEventKeyboardSetUnicodeString`, keycode 0) and keycode events reached TextEdit's focused text area and Calculator ("1" → "13" after a unicode "3") `[probe]`. A keycode-only "4" to Calculator showed up only after the *next* event arrived (display "134" after the following click) — keycode events to SwiftUI/Catalyst-style apps may be processed late `[probe, once; cause unknown]`. **Mouse: no** for a window on another Space: left-click, double-click, drag and scroll-wheel events at the text area's screen point, with and without the `kCGMouseEventWindowUnderMousePointer` / `…ThatCanHandleThisEvent` fields (91/92) stamped with the window number, moved no caret, selected nothing, scrolled nothing `[probe: two TextEdit instances]`. Same-Space-but-occluded: untested today `[belief: AppKit resolves the window for a posted mouse event from the event's window number / the point, so it may work when the window is on the current Space]`. | See left. |
| `CGEventPostToPSN` | Process serial number variant, deprecated 10.11 `[doc]`. | As PostToPid. | Same `[belief]`. | Same `[belief]`. |
| `AXUIElementPostKeyboardEvent` | HIServices, to the app's focused element. | Only that app. | Mixed (section 2.4) `[probe]`. | Independent of Spaces `[probe: TextEdit/WhatsApp windows were on another Space]`. |
| Event taps (`CGEventTapCreate`) | Observe/modify the owner's input. | FERB's owner-input watcher. | n/a (listening). Needs Input Monitoring (listen) `[doc]`. | n/a |

### 4.2 Event details that matter `[doc: CGEvent.h, CGEventTypes.h]`

- **Source**: create events from `CGEventSourceCreate(kCGEventSourceStatePrivate)` so they don't inherit the owner's
  modifier state; set `kCGEventSourceUserData` to FERB's mark so the owner-input watcher ignores them (existing
  `input.py`).
- **Unicode vs keycode**: `CGEventKeyboardSetUnicodeString` types any character independent of layout; keycode events
  (`CGEventCreateKeyboardEvent(src, keycode, down)`) are needed for key equivalents, arrows, Escape, Return, and for
  apps that read `keyCode` (games, some Electron shortcuts). Both reach a background AppKit text view `[probe]`.
- **Modifiers**: `CGEventSetFlags(ev, kCGEventFlagMaskCommand | …)` on the key event; for posted events the flags
  travel with the event, no separate modifier-down event is required `[doc]`. Not probed today for pid delivery
  `[belief: Escape and shortcuts reached WhatsApp in spike 18]`.
- **Mouse**: `kCGMouseEventClickState` 1/2/3 for single/double/triple; drag = mouse-down, `kCGEventLeftMouseDragged`
  moves, mouse-up. `kCGEventScrollWheel` via `CGEventCreateScrollWheelEvent(src, kCGScrollEventUnitLine|Pixel, axes,
  dy, dx)` with `CGEventSetLocation` — delivery to a pid for an other-Space window failed `[probe]`.
- **Window number fields**: `kCGMouseEventWindowUnderMousePointer = 91`, `…ThatCanHandleThisEvent = 92` are set by the
  window server on real events; stamping them on posted events did not change the outcome for an other-Space window
  `[probe]`.

### 4.3 What reaches a non-frontmost app's key window and first responder

- Keys via `CGEventPostToPid`: yes, into the first responder of the app's key window, no activation `[probe]`.
- Keys via `AXUIElementPostKeyboardEvent`: app-dependent `[probe]`.
- Mouse via `CGEventPostToPid`: not for other-Space windows `[probe]`; unknown for same-Space occluded windows.
- `AXPress` and attribute sets bypass events entirely and are the reliable path `[probe]`.

---

## 5. Windows, Spaces, sheets, popovers, menus, capture

- **Window levels** `[doc: NSWindow.Level]`: normal (0), floating, torn-off menu, modal panel, utility, dock, main menu,
  status, pop-up menu, screen saver. `CGWindowListCopyWindowInfo` reports the level as `kCGWindowLayer`; FERB's probe
  treats layer 0 as app windows. The Finder desktop is far below 0 and `Finder`'s only AX window with no Finder
  windows open is that desktop (title "", frame 0,0,1470,956) `[probe]`.
- **Spaces**: `kCGWindowListOptionOnScreenOnly` lists windows on the *current* Space, front to back `[doc]`; windows
  on other Spaces come only with `kCGWindowListOptionAll` and showed `onscreen: false` for every TextEdit/Calculator
  window while the owner was in full-screen Brave/VS Code `[probe]`. Consequences measured: `AXWindows` empty, app
  hit-test returns the menu bar, posted mouse/scroll events don't land; `AXPress`, attribute sets, menu presses, keys
  to pid all still work `[probe]`.
- A window created in the background (`File > New` by AX press) first appears in `CGWindowList` with bounds 0×0 and a
  real frame a moment later `[probe]`; wait for a non-zero frame before computing points.
- **Sheets** are AX children of their window (role `AXSheet`; `AXSheets` lagged) `[probe]`; **popovers** and
  **menus** appear as separate top-level AX windows/`AXMenu` elements under the application element (not under the
  window) `[doc: NSPopover and NSMenu accessibility]`, so a snapshot must include the app's children, not only the
  focused window `[belief for the exact placement]`.
- `CGWindowListCreateImage` is deprecated since 14 `[doc]`; **ScreenCaptureKit** captures one window with
  `SCContentFilter(desktopIndependentWindow:)` "regardless of its position on screen", including occluded windows and
  windows on other Spaces `[doc: ScreenCaptureKit]` — needs Screen Recording (granted). The existing
  `screenshot.py` already uses per-window capture; its behaviour for other-Space windows is `[belief]` until a probe
  confirms.

---

## 6. Apple Events and AppleScript

- Apple events reach an app in the background; nothing activates unless the script says `activate` `[doc]`.
  `close document "Untitled 2" saving no` closed the probe's document from a background TextEdit with the front app
  unchanged `[probe]`.
- With **two instances** of the same app, `tell application id "…"` is ambiguous (it answered with the other
  instance's documents) `[probe]`; close through AX in that case (window `AXCloseButton` → sheet → "Delete" for an
  untitled document, verified gone from `CGWindowList`) `[probe]`.
- `activate` / `reopen` bring the app forward and switch Spaces to its window — forbidden in FERB's AppleScript
  wrapper. `do script` in Terminal / iTerm runs shell commands as the owner — to be added to the forbidden list
  (phase 2) `[doc: Terminal scripting dictionary]`.
- Sending Apple events to another app needs the **Automation** TCC grant per (sender, target); the first use prompts
  the owner `[doc]`. `osascript` from the daemon inherits whatever grant launchd's context has `[belief]`.
- Scriptable apps (TextEdit, Finder, Mail, Calendar, Music, Safari, Chrome) expose object models that are often a
  cleaner background path than AX (e.g. Finder view mode, TextEdit text). Not scriptable: most Electron apps, SwiftUI
  apps without a sdef `[doc: per-app sdef]`.

---

## 7. Launching and activation — what steals focus

| Call | Activates? | Notes |
|---|---|---|
| `open -g -b <bundle>` | No `[doc: open(1) "-g do not bring the application to the foreground"]` `[probe: Calculator and TextEdit launched this way; front app unchanged]`. | Windows land on the current desktop Space when the owner is on one; when the owner is in a full-screen Space they go to the last desktop Space (every probe window was off-screen) `[probe]`. `-j` launches hidden `[doc]`. `-n` makes a new instance (used for the clean TextEdit) `[probe]`. |
| `NSWorkspace.openApplicationAtURL:configuration:` with `activates = false` | No `[doc]`. | Modern replacement for `NSWorkspaceLaunchWithoutActivation` (deprecated) `[doc]`. |
| `NSRunningApplication.activateWithOptions:` | Yes. | macOS 14+ cooperative activation: an app can only activate itself if the current app yields; `activateIgnoringOtherApps` is deprecated and may be refused `[doc: NSRunningApplication.activate(from:options:)]`. |
| Chromium / Electron new window | Chromium activates itself when it opens a new browser window (`chrome://` or `--new-window`) even from a background launch `[belief; the rework plan calls it the "new-window activation bug"]`. | Phase 3 plans FERB's own minimized window per profile to avoid it. |
| `AXRaise` on a window | No app activation, but reorders within the app `[doc]`. | Visible if the app's windows are on the owner's Space. |
| AppleScript `activate` | Yes. | Forbidden in FERB. |

---

## 8. TCC permissions `[doc: TCC services; CGEvent.h; AXUIElement.h]`

| Permission | Gates | How to check |
|---|---|---|
| Accessibility (`kTCCServiceAccessibility`) | Every `AXUIElement*` read/write/action on other apps; posting CGEvents (`CGEventPost*`, `CGEventPostToPid`) `[doc: CGRequestPostEventAccess]`. | `AXIsProcessTrusted()`, `CGPreflightPostEventAccess()` — both True `[probe]`. |
| Input Monitoring (`kTCCServiceListenEvent`) | Event taps that *listen* (`CGEventTapCreate` with listen-only or active taps) `[doc: CGRequestListenEventAccess]`. | `CGPreflightListenEventAccess()` — True `[probe]`. |
| Screen Recording (`kTCCServiceScreenCapture`) | ScreenCaptureKit / `CGWindowListCreateImage`; without it `CGWindowListCopyWindowInfo` omits window **names** of other apps `[doc]`. | `CGPreflightScreenCaptureAccess()` — True `[probe; window names were present]`. |
| Automation (`kTCCServiceAppleEvents`) | Apple events to each target app, per sender. | Prompted on first use; `osascript` to TextEdit worked without a prompt in this context `[probe]`. |
| Full Disk Access | Not needed for any of the above. | |

Grants are per **code signature / designated requirement** of the sender (see the launcher spike notes in
`ARCHITECTURE.md`): the daemon under launchd must carry the same grant as the terminal context that was probed
`[doc]`.

---

## 9. Secure Keyboard Entry

- Any app can call `EnableSecureEventInput()`; Terminal has a menu item for it; password fields
  (`NSSecureTextField`) enable it while focused `[doc: Carbon Event Manager, Terminal]`.
- While it is on, **event taps receive no key events** `[doc: CGEventTapCreate — key events are delivered to taps
  only when secure input is off]`, so FERB's owner-input watcher goes blind to typing (it still sees the mouse).
- Whether `CGEventPostToPid` keys still enter the *secure* app's own field: `[belief]` that they do (the restriction
  is on observation, not injection); untested, and FERB never types into secure fields anyway (`AXSecureTextField`
  values are never read or written — existing policy).
- `IsSecureEventInputEnabled()` tells whether it is on system-wide `[doc]`; the driver should read it before deciding
  a missing keystroke means "the app ignores background keys".

---

## 10. App Nap and timer throttling

- AppKit apps that are not visible, not frontmost and not receiving events are eligible for **App Nap**: timers
  coalesced, I/O and CPU priority lowered, until user activity, an incoming event, or
  `NSProcessInfo.beginActivity(options: .userInitiated…)` `[doc: Energy Efficiency Guide for Mac Apps]`.
- Consequences for FERB: an AX press or posted event counts as activity and wakes the app for that work `[belief]`;
  state reads right after an action can lag — Calculator's menu check mark needed ~1.5 s, a background-created window
  took a moment to get a real frame `[probe]`. Poll with a deadline instead of a fixed sleep.
- Chromium/Electron throttle background renderers (timers to 1 Hz, rAF stopped) when a window is occluded or hidden
  `[doc: Chromium page visibility / occlusion]`; Electron's `backgroundThrottling: false` web preference disables it
  per window `[doc: Electron BrowserWindow]`. FERB can't set that for other apps; expect UI reactions in a background
  Electron app to take up to ~1 s `[belief]`.

---

## 11. Mechanism matrix

"Other Space" = window not on the owner's current Space (the condition every probe today ran under).

| Mechanism | Works in background | Reaches occluded window (same Space) | Reaches other Space | Frameworks honouring it | Caveats |
|---|---|---|---|---|---|
| `AXPress` on a control | Yes `[probe]` | Yes `[probe: the windows were off-screen entirely]` | Yes `[probe]` | AppKit, SwiftUI `[probe]`; Electron/Chromium `[probe, spike 18]`; Qt/Java/Flutter `[belief]` | Semantic only; verify by re-read. Nothing for custom-drawn views. |
| `AXPress` on a menu item | Yes `[probe]` | Yes | Yes | Any app with a menu bar | Items with a modal state (quit review) can wedge the app. |
| `AXValue` set | Yes `[probe]` | Yes | Yes | AppKit text fields/areas, Electron inputs `[probe]` | Whole value replaced. |
| `AXSelectedTextRange` + `AXSelectedText` | Yes `[probe]` | Yes | Yes | AppKit NSTextView `[probe]`; Electron/Chromium editable DOM `[belief]` | Best edit primitive; check `AXUIElementIsAttributeSettable`. |
| `AXFocused` set | Yes `[probe]` | Yes | Yes | AppKit `[probe]`, Electron `[spike 18]` | |
| `AXUIElementPostKeyboardEvent` | App-dependent `[probe]` | Yes | Yes | WhatsApp/Electron yes `[spike 20]`; TextEdit, Calculator no `[probe]` | Deprecated; keycodes only. |
| `CGEventPostToPid` keys (unicode) | Yes `[probe]` | Yes | Yes | AppKit (TextEdit), SwiftUI (Calculator) `[probe]`; Electron (WhatsApp) `[spike 18]` — but spike 20 saw the search box ignore them | Goes to the app's first responder, whatever it is. |
| `CGEventPostToPid` keys (keycode) | Yes `[probe]` | Yes | Yes | AppKit `[probe]`; Calculator processed it late `[probe]` | Needed for Escape/Return/arrows/shortcuts. |
| `CGEventPostToPid` mouse click/drag | **Not for other-Space windows** `[probe]` | Unknown `[belief]` | No `[probe]` | — | Even with window-number fields. |
| `CGEventPostToPid` scroll wheel | **Not for other-Space windows** `[probe]` | Unknown `[belief]` | No `[probe]` | — | |
| `CGEventPost` (HID tap) | No (goes to the owner's focus/pointer) | n/a | n/a | all | Foreground path only; yields to the owner. |
| App-element hit test | Only current-Space windows `[probe]` | Likely `[belief]` | No (returns the menu bar) `[probe]` | all | Check the role of what comes back. |
| System-wide hit test | Returns what the owner would hit `[probe]` | No | No | all | For "would a real click hit the target". |
| ScreenCaptureKit single window | Yes `[doc]` | Yes `[doc]` | Yes `[doc]` | all | Needs Screen Recording. |
| AppleScript (no `activate`) | Yes `[probe]` | Yes | Yes | scriptable apps only | Automation TCC; multi-instance ambiguity. |
| `AXRaise` | Yes but visible | — | Switches nothing itself | AppKit | Reorders the app's windows. |

---

## 12. Recommendations: what the driver should use per primitive, in the background

Ranked; each step falls through to the next only on verified failure (state re-read), and the per-app profile records
which rung worked so the next call starts there.

1. **Press a control** (button, tab, row, link): `AXPress` → `AXPress` on the nearest ancestor that lists it (Electron
   groups) → keyboard: `AXFocused` + Return/Space via `CGEventPostToPid` → *foreground only with
   `allow_foreground=true`*: real click.
2. **Type into a field**: `AXFocused` the field, then `CGEventPostToPid` unicode key events, read the value back →
   `AXUIElementPostKeyboardEvent` (apps the profile marks) → `AXValue` (replace whole value; fine for empty fields and
   search boxes) → foreground.
3. **Edit existing text** (insert at caret, replace a range): `AXSelectedTextRange` + `AXSelectedText` → `AXFocused` +
   arrow/shift-arrow keycodes to pid then typed text → `AXValue` rewrite of the whole value with the edit applied
   (last resort; loses undo granularity) → foreground.
4. **Click at a point** (custom-drawn UI, canvases, games): there is no verified background mechanism. Order: app-element
   hit test to turn the point into an element and use rung 1 → `CGEventPostToPid` mouse events *only if the window is
   on the current Space* (unverified; the profile must learn it per app; always re-read state) → foreground.
5. **Scroll**: `AXScrollToVisible` on the target element → set `AXVisibleCharacterRange` (text) or the scroll bar's
   `AXValue` / `AXIncrement`/`AXDecrement` on the `AXVerticalScrollBar` of the `AXScrollArea` → keys to pid (Page
   Down/Up, arrows, Home/End) with the scroll area focused → `CGEventPostToPid` scroll wheel (unverified) → foreground.
6. **Menus**: menu-bar items by path through `AXMenuBar` + `AXPress` (verified across AppKit and SwiftUI); contextual
   menus via `AXShowMenu` then `AXPress` on the `AXMenu` under the app element (unverified); menu-bar extras via
   `AXExtrasMenuBar`.
7. **Pop-ups, sheets, popovers, dialogs**: enumerate the app element's children and the window's `AXChildren` (not only
   `AXSheets`), press their buttons with `AXPress`; close with `AXCancel` or the Cancel button, never with Escape posted
   blindly (Escape to pid did not dismiss a remote-view Save sheet `[probe]`); pop-up buttons (`AXPopUpButton`):
   `AXPress` then pick the `AXMenuItem` by `AXPress`, or set `AXValue` where settable.
8. **Window / app bookkeeping**: find windows via `CGWindowList` (`kCGWindowListOptionAll`) + `AXFocusedWindow` /
   `AXMainWindow` / `AXFocusedUIElement→AXWindow`, never `AXWindows` alone; read the frontmost app from
   `AXFocusedApplication` with `NSWorkspace.frontmostApplication` as fallback; never `quit`, never `activate`.

---

## 13. Probe results on this Mac (2026-10-02)

`sw_vers`: ProductName macOS, ProductVersion 26.6.2, BuildVersion 25G83. Accessibility: granted. Screen Recording:
granted. Input Monitoring: granted. Owner's app during the runs: Brave Browser (full-screen YouTube) then Visual
Studio Code (full-screen), then Brave again — the owner was active (switched apps, moved the mouse during one
Calculator run). All target windows were therefore on another Space (`onscreen: false`).

Runner: `.venv/bin/python -m ferb.act.mac.probe <bundle> [--pid=N]`. The JSON below is the profile as printed, with the
per-step `steps` array summarised after each.

### 13.1 TextEdit (clean instance launched with `open -n -g -b`, pid 24722)

```json
{
 "bundle": "com.apple.TextEdit",
 "ax_tree_size": 46,
 "has_actions_on_groups": false,
 "ax_press_ok": false,
 "ax_set_value_ok": true,
 "ax_selected_text_ok": true,
 "ax_post_key_ok": false,
 "pid_keys_ok": true,
 "pid_keycode_ok": true,
 "pid_mouse_click_ok": false,
 "pid_mouse_drag_ok": false,
 "pid_scroll_ok": false,
 "menu_press_ok": true,
 "app_hit_test_ok": false,
 "ax_focus_ok": true,
 "front_app_unchanged": true,
 "mouse_unchanged": true,
 "notes": ["could not close 'Untitled 2' through AppleScript (Automation permission?); it is left open unsaved so the owner can discard it"],
 "pid": 24722,
 "launched_by_probe": false,
 "ax_windows": [],
 "cg_windows": [["Untitled", false]],
 "root_window": {"title": "Untitled", "rect": [146.0, 71.0, 586.0, 488.0]},
 "groups_with_actions": 0,
 "tree_truncated": false,
 "app_hit_test_covered_ok": false,
 "system_wide_hit_returns_target": false,
 "file_new_enabled": true,
 "new_window_cg": {"name": "Untitled 2", "onscreen": false, "bounds": {"X": 0.0, "Height": 0.0, "Y": 0.0, "Width": 0.0}},
 "created_document": "Untitled 2",
 "text_area_actions": ["AXShowMenu"],
 "text_area_settable": {"AXValue": true, "AXSelectedText": true, "AXSelectedTextRange": true, "AXFocused": true},
 "ax_selected_text_replace_ok": true,
 "caret_after_pid_click": [20, 0],
 "window_number": 79892,
 "pid_mouse_click_winnum_ok": false,
 "caret_after_pid_click_winnum": [20, 0],
 "selected_after_pid_double_click": "",
 "selected_after_pid_drag": "",
 "pid_mouse_drag_winnum_ok": false,
 "scroll_before_after": [{"scrollbar": 0.0, "visible": [0, 214]}, {"scrollbar": 0.0, "visible": [0, 214]}],
 "pid_scroll_winnum_ok": false,
 "scroll_after_winnum": {"scrollbar": 0.0, "visible": [0, 214]},
 "final_value_head": "line 0\nline 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\nline 10\nli",
 "cleanup": {"ax_close": false, "detail": "no sheet appeared"},
 "cg_windows_at_end": [["Save", false], ["Untitled 2", false], ["Untitled", false]],
 "ax_focused_app_errors": [-25204],
 "front_changes": [],
 "owner_front_app": "ws:com.microsoft.VSCode",
 "mouse_before_after": [[886.8, 784.5], [886.8, 784.5]]
}
```

Steps (all with `front_before` = `front_after` = com.microsoft.VSCode): `set_AXEnhancedUserInterface` → -25208;
`set_AXManualAccessibility` → -25205; `walk_tree` → [46, 0, false]; `hit_test:window centre` at (439, 315) →
app element hit `AXMenuBar` (err 0, pid 24722), system-wide hit `AXGroup` of pid 41154 (VS Code), window covered,
target not on screen; `menu_press File>New` → 0; `ax_set_value` → 0; `ax_selected_text_insert` →
`{set_range_err: 0, range_after_set: [6, 0], set_text_err: 0}`; `ax_selected_text_replace` → value "ALPHA bravo ";
`ax_focus` → 0; `ax_post_keyboard` (191 ms) → no text arrived; `pid_keys_unicode` → "delta " arrived;
`pid_keys_keycode` (kVK_ANSI_E, kVK_Space) → "e " arrived; `hit_test:text area` at (179, 210) → `AXMenuBar`;
`pid_mouse_click`, `pid_mouse_click+window_number`, `pid_mouse_double_click`, `pid_mouse_drag`,
`pid_mouse_drag+window_number` → caret stayed at 20, nothing selected; `pid_scroll` ×5 and
`pid_scroll+window_number` ×5 → scrollbar 0.0, visible range unchanged; `menu_press Format>Text>Show Ruler` → 0.

`ax_press_ok: false` here means only that the ruler toggle had no readable check state to confirm it (the press
returned 0); AXPress itself is proven by the Calculator run and the menu presses.

Cleanup note: `instances == 2`, so the AX path ran; the close button produced the Save sheet but it appeared under
`AXChildren` (role `AXSheet`) and not `AXSheets` within the 3 s the first version waited. A follow-up pressed that
sheet's "Delete" button (buttons: Save, Cancel, Delete) in the background → err 0, "Untitled 2" gone; the probe's
cleanup now checks `AXChildren` and waits 6 s.

An earlier run against the **stuck** first instance (pid 16283, see 13.4) gave the same mechanism results:
`ax_set_value_ok: true, ax_selected_text_ok: true, ax_selected_text_replace_ok: true, ax_focus_ok: true,
pid_keys_ok: true, pid_keycode_ok: true, ax_post_key_ok: false, pid_mouse_click_ok: false, pid_scroll_ok: false,
menu_press_ok: true, front_app_unchanged: true, mouse_unchanged: true`, with `root_window_via_focused_element` used
because `AXFocusedWindow`/`AXMainWindow` were None.

### 13.2 Calculator (launched by the probe with `open -g -b`, pid 21619)

```json
{
 "bundle": "com.apple.calculator",
 "ax_tree_size": 35,
 "has_actions_on_groups": false,
 "ax_press_ok": true,
 "ax_set_value_ok": null,
 "ax_selected_text_ok": null,
 "ax_post_key_ok": false,
 "pid_keys_ok": false,
 "pid_keycode_ok": false,
 "pid_mouse_click_ok": false,
 "pid_mouse_drag_ok": null,
 "pid_scroll_ok": null,
 "menu_press_ok": true,
 "app_hit_test_ok": false,
 "ax_focus_ok": null,
 "front_app_unchanged": true,
 "mouse_unchanged": false,
 "notes": ["Calculator: no scrollable or text-editing target; scroll / set-value / selected-text not applicable"],
 "pid": 21619,
 "launched_by_probe": false,
 "ax_windows": [],
 "cg_windows": [["Calculator", false]],
 "root_window": {"title": "Calculator", "rect": [285.0, 440.0, 230.0, 408.0]},
 "groups_with_actions": 0,
 "tree_truncated": false,
 "app_hit_test_covered_ok": false,
 "system_wide_hit_returns_target": false,
 "calc_buttons_found": {"1": true, "2": true, "clear": true, "display": "‎0"},
 "display_after_ax_press": "‎1",
 "display_after_ax_keyboard": "‎1",
 "display_after_pid_keys": "‎13",
 "display_after_pid_click": "‎134",
 "calc_mode_switch": {"checked_after": "Scientific", "window_before": [285.0, 440.0, 230.0, 408.0], "window_after": [285.0, 440.0, 674.0, 408.0]},
 "cg_windows_at_end": [["Calculator", false]],
 "ax_focused_app_errors": [-25204],
 "front_changes": [],
 "owner_front_app": "ws:com.microsoft.VSCode",
 "mouse_before_after": [[345.8, 603.4], [337.6, 601.1]]
}
```

Reading the display chain: All Clear (AXPress) → "0"; AXPress "1" → "1"; AX keyboard "2" → still "1" (ignored);
`CGEventPostToPid` unicode "3" → "13" (**`pid_keys_ok` is a false negative**: the check expected "123" because it
assumed the AX keyboard had worked); keycode "4" → still "13" after 0.8 s; pid click on button "5" → "134" (the "4"
arrived with the next event; the click itself did nothing). `launched_by_probe` is false in this (third) run because
the first run launched it. `mouse_unchanged: false` is the owner moving the mouse (8 px) during the run; the probe
never posts to the HID tap. Three runs gave identical mechanism results.

### 13.3 Finder (pid 96185, no Finder windows open; read-only)

```json
{
 "bundle": "com.apple.finder",
 "ax_tree_size": 29,
 "has_actions_on_groups": true,
 "ax_press_ok": null, "ax_set_value_ok": null, "ax_selected_text_ok": null, "ax_post_key_ok": null,
 "pid_keys_ok": null, "pid_keycode_ok": null, "pid_mouse_click_ok": null, "pid_mouse_drag_ok": null, "pid_scroll_ok": null,
 "menu_press_ok": null,
 "app_hit_test_ok": true,
 "ax_focus_ok": null,
 "front_app_unchanged": false,
 "mouse_unchanged": true,
 "notes": ["Finder: no front window with view modes enabled; menu press skipped",
           "Finder: acting steps limited to a view-mode toggle (restored); no typing, clicks or scrolls"],
 "pid": 96185,
 "launched_by_probe": false,
 "ax_windows": [""],
 "cg_windows": [],
 "root_window": {"title": "", "rect": [0.0, 0.0, 1470.0, 956.0]},
 "groups_with_actions": 1,
 "tree_truncated": false,
 "app_hit_test_covered_ok": true,
 "system_wide_hit_returns_target": false,
 "finder_view_items": {"checked": "as Icons", "enabled": {"as Icons": false, "as List": false, "as Columns": false, "as Gallery": false}},
 "cg_windows_at_end": [],
 "ax_focused_app_errors": [-25204],
 "front_changes": ["walk_tree: com.microsoft.VSCode -> com.microsoft.VSCode (owner app was ws:com.microsoft.VSCode)",
                   "hit_test:window centre: com.microsoft.VSCode -> com.microsoft.VSCode (owner app was ws:com.microsoft.VSCode)"],
 "owner_front_app": "ws:com.microsoft.VSCode",
 "mouse_before_after": [[347.8, 357.3], [347.8, 357.3]]
}
```

This Finder run predates two probe fixes: `front_changes` here are the `ws:` fallback prefix compared against the AX
reading of the same app (not a real change; fixed), and `app_hit_test_ok: true` was the weak criterion that accepted
an `AXMenuBar` result (fixed; the hit at the desktop centre was in fact `AXMenuBar`). Finder's View items were
disabled with no Finder window open, so no menu press was made.

### 13.4 Side experiments (same day, same conditions)

- **Hit test vs Space**: `AXUIElementCopyElementAtPosition(app, …)` at the centre of the owner's on-screen VS Code
  window → err 0, `AXGroup`, window "FERB"; at a point inside Calculator's other-Space window → err 0, `AXMenuBar`;
  system-wide at that same point → `AXGroup` of pid 41154 (VS Code).
- **`AXEnhancedUserInterface`**: reads `True` on Calculator and TextEdit; `AXUIElementIsAttributeSettable` → (0, True);
  setting with Python `True`, `1`, `NSNumber(True)` → -25208 each time, value still True. `AXManualAccessibility`
  settable → (-25205, False) on both.
- **`AXFocusedApplication`**: err -25204 on several consecutive reads while the owner was in a full-screen Brave,
  and again intermittently under full-screen VS Code; `NSWorkspace.frontmostApplication` and
  `menuBarOwningApplication` answered correctly each time.
- **Escape to pid against a remote-view Save sheet**: `CGEventPostToPid` keycode 53 and `AXUIElementPostKeyboardEvent`
  Escape / Cmd+. → sheet unchanged.
- **AppleScript across instances**: with two TextEdit instances, `get name of every document` under
  `tell application id "com.apple.TextEdit"` answered for the other instance ("Untitled, Untitled 2").
- **The stuck instance (pid 16283)**: the first (crashed) probe run `quit` TextEdit with three unsaved untitled
  documents created by the probe; the quit review's Save sheet stayed, `close every document saving no` → document
  count 0 but the windows ("Untitled", "Untitled 2", "Untitled 3", "Save") remained in `CGWindowList`, `AXWindows` /
  `AXFocusedWindow` / `AXMainWindow` / System Events `windows` all empty, while `AXFocusedUIElement`, `AXMenuBar`,
  AX menu presses, `AXValue`/`AXSelectedText` sets and keys to pid kept working in it. Each later `File > New` left
  another zombie "Untitled 2". **State left on this Mac** (the probe may not quit apps): TextEdit pid 16283 in that
  state, TextEdit pid 24722 with one empty "Untitled", Calculator pid 21619 in Basic mode with a cleared display. All
  three are on the desktop Space. The owner should quit TextEdit 16283 (choose Delete / Don't Save for anything it
  asks; every document in it is the probe's).

### 13.5 Answers to the five questions

(a) **Mouse clicks via `CGEventPostToPid` to a background TextEdit window**: no caret move, no selection, for a window
on another Space, with or without window-number fields; same-Space occluded case not reachable today.
(b) **`AXSelectedText` insertion for editing**: works (insert at a caret range and replace a range), in the background,
on another Space.
(c) **`kCGEventScrollWheel` via PostToPid**: did not scroll an other-Space window.
(d) **App-element hit test under another window**: resolves correctly for a window on the current Space (tested on the
owner's own on-screen window, read-only); for other-Space windows it returns the app's menu bar. Same-Space occlusion
specifically: untested.
(e) **Did anything change the frontmost app?** No, in every run (`front_changes: []` once the `ws:` artefact was
fixed); the pointer moved only when the owner moved it.

### 13.6 What to re-probe when the owner is on a desktop Space

Run `ferb.act.mac.probe` for TextEdit with its window on the owner's Space but behind another window: this decides
whether `CGEventPostToPid` mouse/scroll and the app hit test work for occluded same-Space windows (phase 3's "click at
point" rung). Also worth adding: `AXScrollToVisible` on a table row, `AXShowMenu` + `AXMenu` placement, modifier
combos to pid, ScreenCaptureKit capture of an other-Space window, and an Electron app (`has_actions_on_groups`,
`AXManualAccessibility`) using the FERB test profile of a browser or a throwaway Electron app.

## Owner-verified runs (`ferb watch`, 2026-10-02, macOS 26.6.2, owner in full-screen VS Code)

Three runs, 25 distinct steps, the owner watching: **0 steps moved the desktop, raised a window, or touched the
mouse**; the notification detector and the Accessibility frontmost poll agreed. Facts the drivers rely on:

| Mechanism | Result | Tag |
|---|---|---|
| `open -g -b` for an app that isn't running | starts in the background | [owner] |
| Menu-bar item via AXPress (File > New) | works; new document created | [owner] |
| `AXValue` set on a text area; `AXSelectedText` insert at the caret | works | [owner] |
| Unicode keystrokes via `CGEventPostToPid` (43 chars) | land in the focused text view | [owner] |
| Page Down keycode via `CGEventPostToPid` | scrolls the text view (visible range 0 → 671) | [owner] |
| Scroll-wheel events via `CGEventPostToPid` (window on another Space) | nothing | [owner] |
| Mouse down/up via `CGEventPostToPid` (window on another Space) | nothing (caret unchanged) | [owner] |
| AXPress on Calculator buttons (7 × 6 =) | 42 | [owner] |
| Typed `12+3` + Return keycode to Calculator's process | 15 | [owner] |
| ⌘A via `CGEventPostToPid` to TextEdit in the background | **nothing, and the app beeps**: its Edit > Select All item reports `AXEnabled: false` while the app has no key window, so the key equivalent has no handler | [owner] |
| Right-arrow keycode at the end of the text | the app beeps | [owner] |
| Chromium: `windows.create({state:"minimized"})`, `tabs.create({active:false})`, `debugger.attach` + focus emulation, trusted click / insertText / Enter / mouseWheel, `Page.captureScreenshot`, `tabs.update(url)`, and even `windows.create({focused:false})` (normal state) | all worked, nothing visible | [owner] |

Rules drawn from this: (1) shortcuts in a background app go through the menu item they map to
(`AXMenuItemCmdChar` / `AXMenuItemCmdModifiers`), and only when that item is `AXEnabled`; otherwise the edit is done
through the text attributes (`AXSelectedTextRange`, `AXSelectedText`, `AXValue`); (2) never post arrow keys blind
into a text view; (3) scroll a background text view with Page Up/Down keycodes or `AXVisibleCharacterRange`, not
scroll-wheel events; (4) pixel clicks are a last resort that needs the window on the current Space.

## Phase 3 driver (2026-10-02)

`ferb/act/mac/driver.py` implements §12 rung by rung, `ferb/act/mac/accessibility.py` the one-walk snapshot (§2.3, §5,
§7), `ferb/act/mac/profile.py` the per-app learned profile (`ferb.act.mac.probe --seed` writes a probe result into
it). The owner-verified rules above are code: shortcuts through their menu item only when enabled, no arrow keys into
a text view, scrolling by Page Up / Down keys or the scroll bar, pixel clicks only for a current-Space window and only
after the hit test, the foreground only with `allow_foreground=true`. `tests/test_mac_background_real.py` repeats the
`ferb watch` Mac steps through the driver with the frontmost app polled every 50 ms (0 changes on 2026-10-02).
