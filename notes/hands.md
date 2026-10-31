# Hands (Mac, browser, shell, files): decisions and fix list

Area: `ferb/act/mac/*`, `ferb/act/browser/*`, `ferb/native_host`, `extensions/chromium/*`, `ferb/act/shell/*`,
`ferb/act/fs/*`, `ferb/act/{aftercheck,point,gui_lock,locks,listen,recording,ask,catalog}.py`, `ferb/act/evidence`.
Basis: a read of every line (2026-10-30), scratch tests on fake files and generated sandbox profiles, read-only DB
counts. File:line refs are to commit c6621c3 / 68dc95b (same files).

## Owner decisions (2026-10-30)
1. **One read question per protected folder per task** (escape hatch and file tools): "Let FERB read your Chrome
   profile folder for this task?" covers every read of that folder in the task; writes and admin still ask per
   command. (Live: 11 questions in one visa task, 6 in another, for the same folder; the old per-command shell rule
   asked 46 and 19 times on 10-13/14 and is already gone.)
2. **Minimal shortcut guard**: in-app shortcuts unchanged; `cmd+q`/`cmd+h` go through FERB's safe quit/hide, which only
   acts on apps FERB launched and checks unsaved work (launched.py:113-145); screen-wide combos (`cmd+tab`,
   `cmd+space`, `ctrl+arrows`, force quit) refused.
3. **Tab groups**: one collapsed, labelled group per task holding only tabs FERB opened. FERB never closes a tab it
   didn't open; a borrowed owner tab is never moved into the group or closed and is left where the work finished; a
   FERB tab holding the result is handed over (moved out, left open); others close with the group at task end; a tab
   dragged out becomes the owner's; an owner tab dragged in stays the owner's; closing a FERB tab or the group
   mid-task counts as the owner stopping that part (FERB asks whether to continue); "keep the tabs open" ungroups and
   leaves them; pause keeps the group; loops keep it collapsed; after a reconnect or restart FERB re-adopts its group
   or closes it if the task ended (no orphans); one group per task; a profile with no window uses FERB's minimized
   window. Needs the `tabGroups` permission and a protocol bump (manifest, JS/Python protocol, minimum versions
   together).
4. **Shell changes: scope + snapshots (A + B)** (owner, 2026-10-30; refines notes/gateway.md decision 3).
   - **A.** Terminal commands can't change anything (write, truncate, delete, rename) in the owner's visible folders
     unless the folder is named — in the owner's words, or in the plan line FERB stated and the owner didn't correct.
     Visible folders: non-hidden folders in the home folder, iCloud Drive (`~/Library/Mobile Documents`), and
     `/Volumes`. Programs keep working: hidden folders (`.git`, `.npm`, `.codex`, `.claude`, `.venv`…), `~/Library`
     (minus the protection lists), temp folders and FERB's workspace stay writable. Kernel-enforced by the sandbox,
     after shell expansion. The owner's files change through file tools (backup copies). A refused command tells the
     worker why; it uses file tools or asks which folder.
   - **How "named" works (typos included):** code never fuzzy-matches folder names. The planner resolves what the
     owner meant ("Downlods" → `~/Downloads`) into a structured `folders` field on the task, shown in the plan line
     ("I'll work in ~/Downloads"); code allows exactly those resolved paths. An ambiguous name → one which-question.
     Work starts at once (rule 9); a wrong guess the owner doesn't correct is covered by B and by file-tool backups.
   - **Hidden folders inside visible ones are writable** (owner, 2026-10-30): any path with a component starting
     with "." (e.g. `~/Desktop/repo/.git`) counts as a program folder, so git works in any repo; B covers mistakes.
   - **B.** A fresh APFS local snapshot before each terminal command, skipped only if no command ran since the last
     one (today at most one per 15 minutes, recovery.py:41). FERB says so when a snapshot couldn't be taken (e.g. a
     nearly full disk); snapshots last about 24 hours and don't cover external drives.
   - Same pattern as Codex and Claude Code (shell writes scoped to the project) and the incident write-ups (scoped
     access + snapshots), widened for a whole-Mac assistant.

(Decisions from notes/gateway.md apply here: tool-reported facts, card fields secret, P19 stable identity,
file tools bound by the protection lists, shell scope per decision 4 below, browser lists
from installed apps, terminal detection fails closed)*

5. **Sound is part of background-first** (owner, 2026-10-30): FERB's own tabs are muted by default unless the task
   needs to listen; FERB stopping or changing playback in a tab the owner is using counts as a disturbance fact; a
   general "keep a tab playing while the owner switches away" ability is later.

6. **Bot checks: workers try visible checks with the bot-check ability** (owner, 2026-10-27, recorded 2026-10-31):
   workers use `ferb/abilities/browser_verification` for a visible "verify you are human" check; `browser_handoff` is
   the fallback. 2026-10-31 (visa run cd80f4ee): the worker never read the ability and gave up at the check, while the
   10-30 runs with the same task text read it and got through; nothing made the ability reach the worker. Owner:
   code states the fact and names the ability; no extra prompt wording about what counts as a bypass.

## What exists today (verified)

### Reliability (live tool calls, 2026-09-29 → 10-30)
| Tools | Calls | Errors | Main causes |
|---|---|---|---|
| browser.* / ferb_browser.* | 5,357 | 6.1% (5.2% since 10-24) | profile not matched/connected 110, stale refs 45, reads marked "outcome unknown" 42 of 56, old "no FERB window" 33 |
| mac.click | 121 | 43% | another window over the point, window moved, owner active, stale signature |
| mac.screenshot | 269 | 26% | ScreenCaptureKit "Failed to start stream" 38 |
| mac.act | 943 | 146 + 58 not-ok | focus elsewhere 29, AXValue not confirmed 17 |
| mac.pointer | 49 | 16 | window moved, app changed |
| shell.run | 1,362 | 4 (1 since 10-20) | — |
| fs.* | ~460 | ~30 | missing paths, non-UTF-8 |

### Browser
- Path: daemon ↔ native host ↔ service worker ↔ content script (injected on demand, all frames). Extension→daemon
  HMAC + seq; daemon→extension unsigned; the extension adopts any `paired` message (background.js:201-213); the native
  host's origin is self-declared (auth.py:132-137). Safe only under "same-uid processes are trusted".
- Reconnect orphans task tabs: refs older than `bridge.gen` refused (ext_bridge.py:1807-1808), FERB tabs can't be
  closed, armed tabs disarmed but left open after a daemon restart; orphaned tabs block the extension's self-reload.
- Queued outbox messages are flushed before the new hello and dropped by the daemon (background.js:102,186;
  ext_bridge.py:428-430).
- **press into password/code fields is not blocked** (only `type`: ext_bridge.py:1162, background.js:1129).
- **Read failures become "outcome unknown"** (background.js:309-310 → ext_bridge.py:161-163): 42 of 56 unknowns were
  reads, which then trip P19.
- `list_tabs` and the pending-dialog snapshot return unmasked URLs (background.js:1458, 1464).
- Card fields not secret: `isSecret` = sealed, password, code (content.js:181-189); cc-number/cc-exp/CVV values appear
  in snapshots.
- Window choice auto-picks the first unfocused window and isn't remembered (ext_bridge.py:833-836).
- Typing into an input inside a closed shadow root always fails (`deepActive`, content.js:764-780); pages can detect
  FERB (`ferb-armed` event, `data-ferb-*` attributes).
- Structural facts available: `el.form`, `autocomplete`, `input_type` already read in content.js; `backendDOMNodeId`
  held in ax.js and discarded (usable for stable P19 identity). **No structured source for checkout or bank-page
  amounts**: page text only, bank 3DS pages are usually cross-origin frames (content.js runs in all frames, so often
  readable). The payment decision's amount check is therefore best effort, with the "couldn't read the amount"
  fallback.
- Site-specific code is small: Gmail Atom feed read source, xterm.js detection, the Zomato split-code heuristic
  (generic logic), Google's show-password field, Cloudflare closed shadow roots.

### Mac
- **Pointer input refused in apps with more than one window**: the shot's signature comes from the explicit window's
  walk (driver.py:660-661, 700), `pointer_at` compares a no-window walk (564-566, 908) and `app_window()` returns None
  for 2+ windows (accessibility.py:176-179). Same root breaks `observed_change` and `ensure_window` (waits 10 s).
- **Browser list of 3** (Chrome, Brave, Safari; ax_browser.py:5): Mac snapshots/screenshots of Arc, Edge, Firefox,
  Vivaldi, Chrome Beta are labelled the owner's own app data, not outside web content (tools.py:61-67) — outside
  content treated as trusted. Four browser lists in the code disagree.
- **Key presses into secure fields not checked** (only type/set_value/edit check `n.secret`, driver.py:1540).
- **Terminal detection fails open** when the focused element can't be read (driver.py:1040-1046; policy/terminal.py:97-109).
- **cmd+q / cmd+h / cmd+tab / cmd+space guards exist only in tests** (input.py:248-287); "cmd+tab" was delivered 6
  times on 09-30/10-01.
- Foreground press doesn't raise the window and doesn't check what's under the point (driver.py:1550-1556, 1024).
- `ax_keyboard`: 8-char chunks drop silently on any non-US character ("hello wörld" types "rld"); US keycodes only;
  `fn` dropped; results always True.
- mechanism ignored for select and focus; `ferb watch` step `scroll_window_targeted` crashes (device/watch.py:357-381).
- No AX messaging timeout; many synchronous AX/NSWorkspace calls on the event loop (one hung app stalls the daemon);
  PID typing holds the global GUI lock for the whole text (3,097 chars ≥ 62 s).
- Dead: the marks path, `has_secure_field`, `settable`, `Profiles.prefers/untried`, test-only input guards.

### Shell
- Profile (`sandbox.py:152-253`): deny-list over `(allow default)`; FERB state, FERB.app, Trash, guarded folders
  always denied; Keychain service, LaunchServices, Apple Events denied; network only via the proxy.
- **Process groups survive task end**: `kill()` returns once the shell's own `exit_code` is set (pty_proc.py:215) and
  `end_task`/`kill_all` skip finished processes (manager.py:270, 279), so `cmd &` children live on.
- **Escape-hatch gaps**: `shell.send` has no grant check (an approved `bash` keeps the lift for anything typed later);
  a write lift's allow comes after never-read write denies (lifting `write:/Library` reopens `/Library/Keychains`);
  FERB's own launchd plist is protected only by a liftable rule (rule 10).
- Localhost ports that start listening after a command starts bypass the proxy; no signal rules (a command can kill
  the daemon/proxy); suspected: contained processes inherit FERB.app's Accessibility/Screen Recording grants (a
  `python -c` could post keyboard events into a terminal without the terminal-typing ask) — needs measuring.
- Host records live in memory until forget/close; a crash loses in-flight records. Shell logs never purged (142 MB).
- `ShellManager._procs` grows forever; a 3,001-byte line sent in canonical mode is dropped while reporting
  `sent:true`; `shell.kill` reports kind OPEN_TAB (counted reversible).
- sandbox-exec is deprecated; FERB fails closed if it stops working; semantics could change silently.

### Files
- fd-anchoring, RENAME_EXCL trash moves with dev/inode checks, APFS S1 copies, never-overwriting restore: sound.
- `fs.edit` CRLF → LF; `atomic_text` writes 0600 and loses mode, xattrs, hard links; non-UTF-8 and permission errors
  escape as exceptions (marked "outcome unknown"); `fs.trash` on a symlink trashes the target; `_restore_copy` ignores
  the protection lists; `fs.write overwrite` onto a folder raises; fs I/O runs on the event loop.

### Decision (shell deletes) — implementation notes
- Seatbelt `file-write-unlink`; rename semantics need measuring (likely unlink on source + create on target).
- Superseded by decision 4 (visible-folder scope + snapshots): hidden folders, `~/Library` (minus protection lists),
  temp and the workspace stay writable, so tool caches and coding agents' homes work without a list.
- Would break: git in repos not named in the task (index.lock renames), `sed -i`/`perl -pi` there, `git config
  --global`, global installs under `~` (pipx, uv tool, cargo install, nvm, pyenv), Codex CLI and Claude Code run in
  FERB's shell (write `~/.codex`, `~/.claude` by rename). Overwriting with `> f` is not unlink.

## Depends on
- notes/orchestration.md: the structured `folders` task field (decision 4), pause and wait-for-owner (tab groups:
  closed tab → ask; pause keeps the group).
- notes/gateway.md consumes the facts produced here.
- notes/storage.md decision 5: extension minimum version in the upgrade path.

## Rulings (step 5a, technical)
- Late localhost listeners: a command connects directly only to FERB-sandboxed listeners that existed when it
  started; anything later goes through the proxy (closes the bypass; clients that ignore proxy settings can't reach
  a server started in the same command).
- Visible folders include files directly in `~` and tool folders like `~/go`; a task that needs one names it
  (the plan line), and snapshots cover mistakes.
- After a failed snapshot the next attempt waits 60 s (a hanging tmutil can't add its timeout to every command);
  each result still states that no snapshot was taken.
- `shell.send` into a command started with a lift is refused.

## Rulings (step 5b, technical)
- Secure fields: a key press is refused only when it would put text into the focused password field (a character
  key without ⌘/⌃, Space, or a paste ⌘V, by key or by the Paste menu item); Return, Tab, Escape, arrows and other
  shortcuts still go (submitting a login FERB didn't type isn't typing a password).
- Shortcut guard: ⌥⌘H (hide others) hides the owner's apps, so it is refused like the screen-wide combos; ⌘M stays an
  in-app shortcut. The guard also covers a menu item whose key equivalent is ⌘Q/⌘H (mac.menu, a press on the item, a
  click on it). Screen-wide also: ⌃Space, ⌃⌘Q (lock), ⇧⌘Q (log out).
- `ax_keyboard` removed (live: 23 uses, none confirmed a change; nothing needs it); `focus`/`select` take `ax` only.
- P19 identity (Mac): `mac.act` = the snapshot window's window-server id + the element's AX role path, role,
  subrole, label (its sibling index only when unlabelled), never the snapshot id or the value; `mac.click` /
  `mac.pointer` = the shot's window id + the element the capture's walk found under the point; with no element there
  or one larger than 400 pt, the point on a 24 pt grid of the window. Window titles are left out (a chat window's
  title changes after a send, which would let a repeat escape); a collision only asks the owner.
- Staleness (pointer route): window id + bounds, then the element the app's hit test finds at the point against the
  one the capture's walk found there (same, inside it, or same role at the same frame); when either is unknown, the
  captured window's own walk is compared. `ax` and `foreground` clicks keep their own right-before checks (the hit
  element bound to the window; the point reaching the window), not this content gate.
- Responsiveness numbers: AX messaging timeout 2 s (macOS default 6 s), set globally and on each app element; a walk
  stops after 8 s or 60,000 elements, marked `budget`, and then can't tell about password fields (captures fall back
  to text recognition, L13).
- Foreground element input (press, scroll) is refused when FERB can't tell which window holds the element (no
  window, or two at one place with one title); the point is checked for FERB windows before that.
- ScreenCaptureKit: captures are serialized in-process and a capture failing to start (SCStreamErrorDomain -3811)
  is tried once more after 0.3 s (a read). Live data: the 38 failures came singly, between successes on the same
  window seconds apart, with no other FERB capture span overlapping; the cause isn't visible from code.

## Rulings (step 5c, technical)
- Tab groups: one group per task tree per window (the task's next tab opens in the window its group is in, so normally
  one); grey, labelled "FERB · <first words of the request>" (about 30 characters, redacted), collapsed when made and
  never expanded by FERB (a tab added to a collapsed group keeps it collapsed). A popup in a popup-type window can't
  be grouped and stays ungrouped (still FERB's). A tab opened from a borrowed owner tab is released like its opener,
  so it isn't grouped either. When the last tab FERB opened leaves a group, the owner's tabs dragged into it are
  ungrouped (left open) and the group is forgotten. An owner who renames the group makes its tabs theirs at the next
  hello (the title check that guards against reused ids).
- Result tab: `browser.keep_open` (existing) is how a worker marks the result: the tab leaves the group, open. "Keep
  the tabs open": `_TabDriver.keep_tabs_open(task)` (step 8c wired the owner's words: the planner's `keep_tabs_open`
  on the task, or in a correction's task, reaches both drivers through BrowserPort.keep_tabs_open); pause: the daemon-supplied
  `task_paused` (default False; step 8) makes `release` close nothing.
- Owner's stop: `browser.owner_stopped` {task_id, tab, profile, borrowed, part: "tab" | "group"} when a tab FERB holds is
  removed without FERB closing it (part "group" when no other FERB-opened tab of that tree is left in the profile);
  `browser.owner_took` when the owner moved a FERB tab out of its group (the extension lets go of it and sends
  `taken`). Both critical bus topics; step 8 turns them into "ask whether to continue".
- Tab identity: the extension keeps its records in chrome.storage.local (survive worker restarts and extension reloads;
  cleared on `runtime.onStartup`, a new browser session, whose reused ids mean nothing). Each hello lists the records
  still valid (tab open, still in its titled group); the daemon re-adopts its tabs (new generation, element ids stale),
  drops ones not listed, adopts listed tabs of a running task it doesn't know (after its own restart), and closes
  (FERB-opened) or releases (borrowed) tabs of ended tasks. The extension's `close` closes only a tab its records hold
  as FERB-opened. A self-reload waits only for work in flight (a command, a held download, a gesture, an attached
  debugger while a connection could drive it), not for open task tabs.
- Signing: welcome and cmd are {type, seq, body, mac}, HMAC-SHA256(pairing key, "<nonce>|<seq>|<type>|<body>") with the
  hello's fresh nonce and a per-connection counter; an assembled command's parts are each verified and the envelope's
  MAC again. Unsigned replies (refused, pairing_required) change no page. A command that fails verification is dropped
  without a reply (echoing its unverified id could fail a real command early), so the daemon times out: unknown, the
  safe side.
- Native host: the daemon takes a connection only when the native host's parent process runs a browser app FERB knows
  (`peer_refusal`, the .app's CFBundleIdentifier through libproc), and a paired profile's hello only from that
  browser's own app. Same-uid limit: any process of the owner's user can start the browser binary with its own
  profile, or read a profile's pairing key from its extension storage; same-uid processes stay trusted.
- Reads: list_tabs, snapshot, wait, screenshot(_part), record_frame, frame_origin, read_source, window_secrets, find,
  result_part, ping, point, and dialog without a decision: an error, a timeout or a dropped connection is a known
  failure (DriverError), never OutcomeUnknown.
- Outbox: messages wait unsigned until the welcome and are signed when posted (at most 5000 kept while refused; the
  oldest go).
- P19 identity: backend node id first (stable while the node lives, so a re-snapshot of the same page matches), DOM path
  when the accessibility pass gave none (screenshot points, a tab without the debugger). A re-render that replaces the
  node escapes the identity; a shifted DOM path can collide (only asks the owner).

## Rulings (regression 2026-10-31, macOS 27.2, technical)
- **Regression:** after the update to macOS 27.2 (Darwin 27.2.0) FERB refused every local port ("FERB couldn't
  inspect the local TCP sockets"); 6 shell/proxy tests failed on clean 41ef7dff. Cause, measured: the kernel's
  socket lists (`net.inet.{tcp,udp}.pcblist[_n]`) now hold only the caller's own sockets for an ad-hoc-signed Python
  (FERB's ~/.ferb/venv, the checkout .venv, Homebrew's) and for any program it starts, so `netstat -anv` exits 0 and
  prints nothing. The same netstat from a terminal, or exec'd in place of that Python, lists everything; an
  ad-hoc-signed copy of netstat lists nothing. Apple's rule isn't documented; it behaves like Local Network privacy's
  attribution of a tool to the third-party program that ran it.
- **Source now (ferb/act/shell/sockets.py):** libproc in FERB's own process, no subprocess (`proc_listpids`,
  `proc_pidinfo PROC_PIDLISTFDS`, `proc_pidfdinfo PROC_PIDFDSOCKETINFO`, SDK `struct socket_fdinfo`, 792 bytes;
  an answer of any other size is no reading). It sees every process of the owner's user and none of another
  user's or root's. The proxy route (`LocalAccess`) uses only it, about 2 ms per reading: the exact connected
  server socket must belong to an inspectable process, so a root or other-user server is refused.
- **Every user's listeners for the profile route:** the shell profile's direct ports (a contained server's port,
  a pinned port) need every listener on that port, or another user's listener on the same port number (other
  address family) would be reachable around the proxy. `sockets.complete()` adds what `/usr/bin/nettop -L 1 -n -x
  -J state` lists for processes libproc can't inspect, each unnamed (pid None: outside, never pinned). nettop is
  Apple's, entitled, and still lists every user's sockets when FERB starts it. It counts only when it lists every
  listener libproc found (a nettop that sees less, the way netstat does now, is no reading: nothing direct, only the
  proxy) and only when every line parses (a program name with a line break makes it unreadable, never misread).
  About 9 ms per profile.
- **Accept wait:** a connection the server hasn't accepted yet is in no descriptor table, only counted in its
  listener's queue (libproc, measured). `LocalAccess.connected` re-reads while a visible listener on that port has a
  queued connection, at most `ACCEPT_WAIT` (30 s, the proxy's own connect timeout), then refuses; with nothing queued
  on a visible listener it refuses at once. netstat had named a queued connection after its listener.
- Sockets shared by several processes are listed once per process, and each must pass (stricter than netstat's one
  last-user pid).
- **FERBNotch:** macOS 27.2 never calls `updateTrackingAreas` in a test window (measured: plain NSView too, on or
  off screen, NSApp running or not, after add, move, resize, scroll), so `LatestSourceHoverTests.testFullNotchSource…`
  found no tracking area. `PreviewHoverTrackingView` now adds or removes its inVisibleRect area itself when it joins a
  window, changes frame, or its clip view scrolls. The pill's hover in the app already ran through the panel's
  mouse-moved monitors and `PreviewHoverRouter`; whether the live app gets AppKit's call wasn't measurable without
  moving the owner's pointer (other overrides: PanelCoordinator, FerbIndicator, PanelResizeOverlay).

## Measurements (step 0, 2026-10-30)
- **Sandbox semantics, measured with `sandbox-exec` on scratch folders:**
  - `deny file-write-unlink` blocks unlink, rmdir, renaming a file out of or within the folder, and renaming onto an
    existing file; it still allows creating files, moving a new file in, truncating and appending.
  - So decision 4 ("no changes in visible folders") needs `deny file-write*`, which blocked all ten operations.
  - Exception for hidden folders works with a later `allow file-write*` regex on any path component starting with "."
    (last match wins): `.git/index.lock` creation and its rename onto `.git/index` were allowed.
- **Shell commands and FERB's permissions:** commands are spawned with `subprocess.Popen` and no responsibility
  disclaim (pty_proc.py:64-66), so they run with the grants of the app that started them. Measured from a granted
  parent: a child under a FERB-like sandbox **could post input events** (a 1 px cursor move, restored) and read the
  screen. Adding `(deny mach-lookup (global-name "com.apple.windowserver.active") (global-name
  "com.apple.windowserver"))` blocked both (no cursor move; screen capture returned nothing) while git, python and
  ordinary commands kept working. **Decision (technical, keeps Full Disk Access for the shell):** add that rule to the
  shell profile, so no script can type into a terminal or capture the screen around the always-ask rules. (The
  owner's own run of the probe was in Terminal, so it showed Terminal's grants, not FERB's.)

## Fix list
- [ ] macOS 27.2: AppKit stopped calling `updateTrackingAreas` in the test process (2026-10-31). The source pill now
      reconciles its own tracking area; the other overrides (PanelCoordinator.swift:85, FerbIndicator.swift:94,
      PanelResizeOverlay.swift:68) are checked live in step 13 (hover-to-open, resize) and get the same pattern if
      the live app is affected.
- [x] Bot-check fact (decision 6): when a page shows a bot-check widget (a structural fact, e.g. the challenge frame's
      origin, reported by the extension/snapshot like a field's kind), the tool result says so and names the
      `browser_verification` ability. No prompt line about bypasses (owner).
      (Step 9b: `PageSnapshot.frame_urls` from what the extension already reports (each child frame's address, each frame
      element's `src`; `ext_bridge.frame_urls`); `snapshot.bot_check` matches them against `CHALLENGE_FRAMES` and the
      snapshot result starts with `bot_check` {widgets, frames, ability: browser_verification, read: "abilities.read
      name=browser_verification"}. No protocol change, so no extension reload; no prompt line. tests/test_perception_9b.py.)
- [x] One extension protocol bump covering tab groups, card fields secret, and daemon→extension signing/pairing
      (manifest, JS/Python protocol and minimum versions together). (5c: protocol 18, extension and minimum 0.16.0,
      `tabGroups` permission; the `rest`/`show_tab` version gates removed. The owner reloads the extension once.)
- [x] Shell: localhost ports opened after a command starts must not bypass the proxy; deny signals to processes
      outside the task's group (a command can't kill the daemon or proxy). (5a: direct only to ports whose listeners
      were all contained at command start, plus pinned ports; anything later only through the proxy, which now
      checks contained listeners too and gets NO_PROXY empty; `(deny signal)(allow signal (target same-sandbox))`,
      measured: own children/setsid descendants yes, other programs and other FERB commands no.)
- [x] Mac: PID typing releases the GUI lock between chunks. (5b: `RealInput.to_process(release_every=8)`; every
      post runs off the event loop.)
- [x] Browser: remove page-detectable markers (`ferb-armed` event, `data-ferb-*` attributes) where possible.
      (5c: hook event names and the linking/stamp attributes are random per service-worker start (protocol.js
      FERB_TOKEN); the linking attribute leaves the page right after each snapshot. Remains: the page hook's patched
      `window.open`/`click`/`dispatchEvent` are visible as non-native (toString) in a task tab while FERB drives it, the
      popup's blank `about:blank#ferb-…` URL, attributes visible to a MutationObserver during a snapshot, the 1280×900
      viewport and focus emulation in FERB's own window.)
- [x] Investigate the top failure causes: profile not matched/connected (110), ScreenCaptureKit "Failed to start
      stream" (38). (5b: ScreenCaptureKit part done, see Rulings; live check in step 13 whether -3811 survives the
      retry. 5c: read-only span counts: 46 "no profile called 'chrome scaler' / 'chrome:scaler'" (only "chrome/x" and
      quoted forms resolved), 2 'Brave Browser' (a browser's name), 15 "select a profile" (no profile given: the
      worker's choice, left), 23+16 "paired but not connected"/old-protocol refusals (before the 10-26 wake fix and
      old extensions). Fixed: `profiles.resolve` takes "<browser>[ :/“]<name>", the browser's label/app name, and
      compares without invisible marks; `profiles.running` used `pgrep -x` (16-char process names: Chrome Beta/Canary,
      Edge channels never looked running). Drops: daemon restarts and MV3 worker deaths; tabs no longer go stale on
      them (readoption).)
- [x] One browser list from installed apps (bundle ids) used by Mac sources, policy, profiles and AX re-read.
      (5b: `ferb/act/mac/browsers.py`: known bundle ids/vendors/names + Launch Services http/https handlers, cached
      60 s; replaces ax_browser, confirm.BROWSER_APPS, profiles' list and scan.py's table; names compared without
      invisible marks. sense/batches.py's name→bundle map is not a browser test and stays.)
- [x] Secret-field rule (password, one-time-code, cc-number/cc-csc/cc-exp) applied to type, press and fill, in the
      browser and Mac; secret values never captured; boxes masked. (5b: Mac done: key presses that enter text and
      the Paste item refuse into a secure field, `accessibility.secure_element`. 5c: content.js `fieldKind` is the
      one rule (cards by autocomplete, inputs and expiry selects); a text-entering press is refused in content.js,
      background.js (`entersText`, also where focus lands) and ext_bridge (`enters_text`); a login code is never
      sealed into a card field; `field_kind` reported.)
- [x] Structural facts reported: form submit + action host/method, field types, focused element/app that received
      input, secure field, page URL; best-effort page amounts. (5c browser: `_TabDriver.act_facts` before the act
      (ActionSpec.facts) and the page's own submit events after it (ToolResult.facts); new field `frame_host` for an
      input in a cross-origin frame (what "bank page" can mean structurally); amount = the clicked control's label
      amount, else exactly one structured page amount (price metadata, JSON-LD); merchant = JSON-LD seller or
      og:site_name. JS fetch() submissions fire no submit event: only network observation could see them, later.) (5a: the shared shape `ActionFacts` in
      ferb/core/types.py, recorded on the call's span; fs tools report writes/deletes. Browser/Mac reporting: 5b/5c.
      5b: Mac tools report target role/subrole, secure field, received_by (read after input), terminal_target and
      page URL in a browser: `ActionResult.observed` → `ToolResult.facts`; new ActionFacts fields target_role,
      target_subrole, page_url. 6b: the gateway raises questions from them (notes/gateway.md Rulings (step 6b)); new
      field `tab_chain` (the first task tab of the chain, from `_Tab.opener`), for one payment per tab chain.)
- [x] Stable P19 identity: browser page URL + backendDOMNodeId/path; Mac bundle + window + element path. (5b: Mac
      done, capability `ledger_args` read by gateway `hashed_args`, see Rulings. 5c: `BrowserAct.ledger_args` and its
      click content use `element_identity`: page origin+path, child frame's, role, backend node id else DOM path.)
- [x] Browser: connection-independent tab identity (no orphans); outbox flushed after welcome; read errors = failed;
      mask `list_tabs`/dialog URLs; remember the chosen window; closed-shadow `deepActive`; sign daemon→extension and
      accept `paired` only during pairing. (5c: see Rulings (step 5c); plus the native host's starter checked by app
      bundle, `_pair` rolled back on error, pointer.js reads `disabled`, one `maskUrl` (mask.js), one key table.)
- [x] Mac: staleness check by window id + bounds + hit element (fixes multi-window); AX messaging timeout and time
      budget; AX calls off the event loop; terminal detection fails closed; key presses check secure fields; one
      foreground path that raises the window and checks the hit target; remove the ax_keyboard route or fix it;
      mechanism honoured or rejected for select/focus; fix `ferb watch`; remove marks and dead helpers. (5b: see
      Rulings; `observed_change`/`ensure_window`/`window_named` fixed for 2+ windows; `ferb watch`'s step was already
      gone; removed marks, `has_secure_field`, `settable`, `describe`, AX keyboard, `Profiles.prefers/untried/seed`.)
- [x] Key-combo guard (decision 2). (5b: `input.screen_wide`/`quit_or_hide` in `driver._guarded_combo` for every
      route; ⌘Q/⌘H through `LaunchedApps.quit_now/hide_now`, only the task tree's own launched app, quit only with no
      unsaved work.)
- [x] Tab groups (decision 3), replacing connection-scoped tab ownership. (5c: see Rulings (step 5c).)
- [x] Per-task folder read lift (decision 1), shared by the escape hatch and file tools. (5a: one key per rule per
      tree, `confirm.read_lift_key`; 6b: both are Need producers of the gateway's one step, `_lift_needs` and
      `_escape_need`.)
- [x] Shell: kill the whole process group at task end regardless of exit; prune `_procs`; `shell.send` respects
      escape grants; never-read write denies after any lift; FERB's launchd plist never liftable; correct `shell.kill`
      kind; durable host records; purge shell logs; report partial/dropped sends. (5a: after the leader exits the
      group is signalled only if every member is contained and the id isn't another command's; sends into a lifted
      command refused; both FERB LaunchAgents are FERB_SERVICE, never lifted; kind `stop_process`; first connection
      per host journaled `shell.destination`; logs purged after evidence_days; over-long canonical lines refused.)
- [x] Measured sandbox rename semantics (see Measurements).
- [x] Shell profile denies the window server (blocks event posting and screen capture from commands); measured.
      (5a: shell profile only; mac.applescript's unchanged.)
- [x] Shell scope + snapshot (decision 4): sandbox denies writes/unlink/rename in visible folders except named
      ones; snapshot before each command. (5a: `ToolContext.folders` from `Gateway.task_folders`, empty until step 8's
      planner field; non-hidden files directly in ~ count as visible too; a failed snapshot is retried at the next
      command and stated in the result.)
- [x] Files: preserve mode/xattrs/newlines on write and edit; catch decode/OS errors as plain errors; trash acts on the
      link; restore honours protection lists; fs I/O in a thread. (5a: APFS clone then rewrite, else mode+xattrs
      copied; a hard-linked file is rewritten in place; `ferb restore` checks only the never-lifted rules.)
- [x] One protection check shared by shell and file tools, with the lift rule of gateway decision 2 and decision 1 here.
      (5a: `Protection.fs_rules`; `fs_reads_never_read` gone.)
