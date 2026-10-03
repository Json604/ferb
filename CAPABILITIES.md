# What FERB can do (0.21.0)

A checklist for trying FERB out. Each row gives an example request, what it needs, how it is verified, and known
limits. Every row was checked against the code or the recorded runs on 2026-10-02.

**Verified**
- **Owner-verified run**: the owner watched it happen (`ferb watch`, a session recorded in `CHANGELOG.md`, or the
  probe sections of `docs/PLATFORM_MACOS.md` / `docs/PLATFORM_BROWSER.md`).
- **Real test**: a `pytest -m real` test against a real model, app or browser (the test file is named).
- **Offline test**: covered by the offline suite (fakes for models, browsers and the Mac).
- **Unverified**: should work from how it's built; no test or run covers it.

**Needs**
- **Mac** = Accessibility granted to FERB's interpreter. **Screen** = Screen Recording.
- **Paired** = a paired Chrome/Brave profile (connected or wakeable) with the FERB extension (0.9.0).
- **FERB's browser** = FERB's own Chrome profile (`ferb setup --identity`), paired.
- **Folder** = you said yes to FERB using that folder (asked once).

When something is missing FERB says so ("I can't do that right now. Unavailable: …") with the fix.

## Chat

| Capability | Example request | Needs | Verified | Known limits |
|---|---|---|---|---|
| Talk, answer, introduce itself | `hi, who are you and what can you do?` | – | Real test (`test_ux_real`) | Plain conversation never starts a worker |
| Answer from an analysis of your Mac | `what apps take the most space on my mac?` | Mac, shell | Real test (`test_ux_real`) | – |
| Durable conversation and task retry | `ferb chat`, `--new`, `--resume ID` | – | Offline daemon restart and legacy recovery (`test_persistent_chat`) | Redacted context, real task IDs and pending questions survive; approvals do not; unknown effects block retry |
| One question when unclear | `send it to him` | – | Offline test (`test_supervisor`) | One question; your next message continues the request |
| Continuity across chats and restarts | `what were you doing earlier? did you finish?` | – | Real test (`test_ux_real`) | Sees the recent conversation and tasks from the journal |
| Tasks outlive the chat window | close chat mid-task, reopen later | – | Offline test (`test_ux`) | A macOS notification goes out; the answer shows next time |
| Stop a task | Ctrl-C while it works | – | Real test (`test_ux_real`) | A send held for undo is cancelled too |
| Pasted text is data, not instructions | paste an email, then `summarize this` | – | Offline test (`test_chat_cli`, `test_answers`) | A whole-message paste asks whether it's your request; `[policy] pasted_text_is_data = false` turns the rule off |
| One worker per request, gated at the action | `search WhatsApp for the syllabus sheet and tell me the gym timing` | – | Offline test (`test_acceptance_one_worker` replays eight real failures; `test_supervisor`, `test_gateway`) | The worker has every tool its identity allows; each acting call declares an effect that FERB raises to the control's floor |
| A worker's question in chat (`owner_ask`) | `use my browser` with two matching profiles | – | Offline test (`test_acceptance_one_worker` 8, 8b) | Numbered options or your own words; Enter declines; declined at once during a scheduled run |
| See what a task did | `ferb trace` / `ferb trace a1b2` | – | Offline test (`test_cli` golden) | Shows each tool call's arguments, declared effect, reason and result |
| Speak replies | `ferb chat --speak` | – | Offline test (`test_unit1_speed_voice`) | The Mac's `say` voice |
| Inline approvals | a send to someone you didn't name | – | Real test (`test_ux_real`, careful mode) | One yes covers the same effect and target for the rest of the request; a question expires after 10 min (1 h unattended) |

## Mac

| Capability | Example request | Needs | Verified | Known limits |
|---|---|---|---|---|
| List apps, read a window | `what's open in Finder right now?` | Mac | Real test (`test_mac_real`, Finder) | Password fields are never read |
| Snapshots show what's really there | (automatic) | Mac | Real test (`test_mac_background_real`: TextEdit text area, close button, the save sheet's Delete; Calculator's buttons); offline `test_mac_snapshot` | A control is anything that exposes an action (pressable rows, groups, cells, images included), with role, value and disabled / selected / expanded state; headings; an open sheet, dialog, pop-up menu or popover is listed with its place; the app's windows are listed and one can be read by title or id; truncation says why |
| Window image with controls numbered | (automatic with the screen scope) | Mac, Screen | Offline test (`test_mac_vision`); the capture path is real-tested (`test_mac_vision_real`) | Not for browsers; no image for excluded apps or windows showing a password field; `[mac] snapshot_image = false` turns it off |
| Read an app on another desktop or full screen | `open whatsapp and tell me the top chat` | Mac | Offline test (`test_ux`); probe (`PLATFORM_MACOS.md` §2.3: `AXFocusedWindow` reaches other-Space windows) | – |
| Operate an app from the background | `in TextEdit, type 'hello' in the open document` | Mac | Owner-verified run (`ferb watch`, three runs, 0 of 25 steps moved anything) + real test (`test_mac_background_real`: new document by menu, typed, edited in place, scrolled by keys, 7 × 6 = pressed, frontmost app polled every 50 ms never changed) | Rung order per primitive (`ferb/act/mac/driver.py`): press = Accessibility press → the enclosing control → focus + Return; type = keystrokes to the app's process, read back → Accessibility keyboard where the app's profile says it works → the box's value; edit = exact range through the selection → whole-value rewrite; key = a ⌘ shortcut runs its menu item and is refused while the app reports it disabled; escape cancels the open sheet / dialog / menu, never blind; scroll = explicit container scrollbar → text-view-only Page keys; bounded steps, progress checked; select = press the pop-up, then its item; typing only when the box really has focus |
| Learn what works per app | (automatic) | Mac | Offline test (`test_mac_snapshot`, `test_background_input`); probe runs on TextEdit, Calculator, Finder (`PLATFORM_MACOS.md` §13) | Each mechanism's landed / failed counts per app in `~/.ferb/device/apps/<bundle>.json`; three consecutive misses skip it until a success is recorded (e.g. a new probe seed); no timed retry; `python -m ferb.act.mac.probe <bundle> --seed` seeds it; nothing per app is written by hand |
| Foreground only by leave | an app that ignores every background way | Mac (Input Monitoring if needed) | Offline test (`test_background_input`) | The result says what failed; only a call with `allow_foreground=true` brings the app in front, once per stretch; your first key / click is held back; your app comes back after 3 s idle or when you touch the Mac |
| Open an app | `open Calculator` | Mac | Owner-verified run (`ferb watch` M1, M10) + offline | Background launch; reopens a closed window from the app's menu; excluded apps refused |
| Menus | `which menu has the search command?` | Mac | Owner-verified run (`ferb watch` M2) + offline | `mac.menu` lists or presses menu-bar items in the background |
| Electron apps (Slack, Discord, VS Code, WhatsApp) | `open slack and check my messages` | Mac | Offline test; spikes 18 / 20 probes on WhatsApp (`PLATFORM_MACOS.md` §1) | FERB asks each for its full Accessibility tree and waits for the main window; not re-probed with the new driver |
| AppleScript inside one scriptable app | `make a note in Notes titled 'groceries'` | Mac, Automation for that app | Offline test (`test_mac_shell`) | One app per script; no `activate`, System Events, shell, `do script` or nested `tell` |
| Screenshot one window, click by position | `in <app>, click the blue Export button` | Mac, Screen | Real test (`test_mac_vision_real`: window-only capture, click lands within 2 pt, covered point refused); offline for the background rungs | The point is first turned into the control under it (the app's own hit test; current-desktop windows only) and pressed through Accessibility; a click posted to the app alone only for a window on the current desktop, kept if something changed; the real mouse only with `allow_foreground=true`; windows showing a password field aren't captured; images never saved |
| Exact native pointer gestures and wheel scrolling | `mac.pointer`: click, dblclick, rightclick, hover, drag, scroll | Mac, Screen | Offline routing/batching/cancellation; owner verified WhatsApp wheel scrolling on another Space without disturbance; other gestures/apps pending | Optional SkyLight window-addressed transport, legacy same-Space fallback; fresh image; no explicit focus transfer; app delivery and lack of disruption must be owner-verified |
| Yield to you | move the mouse while FERB clicks in front | Mac | Real test (`test_mac_real`: marked vs unmarked input) | Only matters in the foreground |
| Pick the app or the site for a service | `video call Rahul on WhatsApp` | the app installed | Offline test (`test_routes`) | The model decides from an environment line (installed, paired, running); no route picker in code |
| Calls and messages in Mac apps | `message Priya on WhatsApp: running late` | Mac | Offline test (`test_contacts`, `test_acceptance_one_worker` 4) | Contact must have been on screen; several matches → "Which Priya?"; the Send button, never Return; 10 s undo; a second message to the same person asks |
| No arrow-key editing, app switching or quitting | (automatic) | Mac | Owner-verified run (`ferb watch`: ⌘A and an arrow key beep in a background TextEdit) + offline | Text is edited by range or replaced whole; ⌘Tab, Control+arrows, ⌘Space, ⌘Q / ⌘H / ⌘M and AppleScript `activate` are refused |

## Browser (your paired profiles)

FERB's own profile is the default for web work. Your profiles require explicit browser/profile direction, such as
"in my Brave" or "my Work profile". Owner-account details alone do not grant personal-browser scopes. A named
profile never silently becomes another; a disconnected one reports its connection problem. The existing identity
split still prevents FERB's profile from holding owner accounts; unsupported login requests must be clarified.

| Capability | Example request | Needs | Verified | Known limits |
|---|---|---|---|---|
| Open and read pages in your signed-in profile | `open <site> in my <profile> profile and tell me what's on it` | Paired | Real test (`test_ext_real`, local pages, spare profile) | Tabs open inactive in FERB's own minimized window of that profile |
| Background by construction | any browser task | Paired | Owner-verified run (`ferb watch` B1–B4) + real test (`test_browser_lab_real`: throwaway Chrome, frontmost app never changed) + offline (`test_browser_background`) | One FERB window per profile, born minimized and kept; task tabs never active; a debugger session attached on a tab's first action and kept for its life; FERB never un-minimizes or focuses its window |
| Click, type, press, hover with real input | `search for 'standing desk' on <site>` | Paired | Owner-verified run (`ferb watch` B5–B7) + real test (`test_browser_lab_real`) | Input is trusted (debugger-dispatched) by default; `page_mode=true` sends scripted events; `<select>` and scroll are page mode; password / code fields are never typed into |
| Scroll the page or one container | `keep scrolling the comments` | Paired | Real test (`test_browser_lab_real`); offline | A real wheel event at the scroller's centre; snapshots list `scrollers` with position and `at_end` |
| Snapshots with viewport semantics | (automatic) | Paired | Offline test (`test_browser_background`) | `in_view`, `offscreen`, `occluded`, `container`, states; open shadow roots walked; iframe rects composed; Accessibility-first roles, names and ordering, with DOM-backed identities and redacted field metadata; 400 elements, in-view first |
| Owner login codes | `browser.login_code` | Paired, interactive CLI | Offline integration (`test_owner_login`, `test_chat_cli`); real site unverified | Hidden input, exact HTTPS frame origin and sensitive non-password field; sealed fill; no code in model results or database logs |
| JavaScript dialogs | a confirm after a click | Paired | Real test (`test_browser_lab_real`); offline | Dismissed by the extension unless the act declared `expect_dialog`; `beforeunload` accepted only while FERB leaves the page; reported with the next snapshot |
| Uploads | `attach report.pdf to the form` | Paired | Real test (`test_browser_lab_real`); offline | `browser.act upload` with absolute paths of existing files; top frame only |
| Exact browser pointer gestures | screenshot → point → act, with two points for drag | Paired | Real throwaway Chrome: canvas, cross-site frame, slider; offline cancellation and stale-image refusal | One image, 30 seconds; same-frame drag; exact pixels; no OS cursor; inspect result after every gesture |
| Screenshots and visual target resolution | `browser.screenshot`, `browser.point` | Paired | Real test (`test_browser_lab_real`) | Exact task-tab JPEG; point returns a normal element id for the policy-gated act tool. Sensitive/incomplete inventories refuse capture |
| Choose the profile when you didn't name one | `check my email` | Paired | Offline test (`test_ext_bridge`) | Open tab → your trail → default browser → ask |
| The profile you name, and only that one | `open my school gmail in chrome …` | Paired | Owner-verified run (0.12.0) + offline (`test_named_browser`) | Paired but disconnected → the wake service is tried; an unavailable named profile never silently becomes another profile |
| Install + pair the extension in a profile | `ferb browsers pair school` | Chrome/Brave | Offline test (`test_named_browser`) | Load unpacked by hand; FERB opens the page, copies the path, waits, pairs |
| Wake a browser without moving you | a browser task with Chrome closed | Paired | Owner-verified run (spike 19, 0.11.0) | `open -g --no-startup-window`, then a wake page; if a browser still jumps forward, your app is put back |
| Use a tab you already have open | `in my brave, keep scrolling the current youtube tab` | Paired | Real test (`test_tabs_real`, needs a "FERB Test" profile) | FERB lets go afterwards and never closes it; its real viewport is kept |
| Check the worker's existing browser tab | verify a last-comment result | Paired | Offline chat/gateway regression; live corrected checker pending | Registered read-only checker reuses its parent's aliases and current page; original ownership and other-task isolation retained; inaccessible evidence is unknown |
| Account check before consequential actions | a send where two accounts are signed in | Paired | Offline test (`test_ext_bridge` 21.12) | Extractors for Gmail and YouTube; other sites ask once per site + profile |
| Calls in web chat, contacts checked | `video call Rahul` | Paired | Real test (`test_contacts_real`, local fake chat page) | The worker sometimes stops to ask instead of letting FERB ask |
| Account and security pages always ask | `turn on 2FA in <site> settings` | Paired | Offline test (`test_ux`) | Every non-reversible action there is `destroy` |
| Read your web email | `what are my unread emails in Gmail?` | Paired | Unverified | Owner-requested OTP login has a hidden CLI handoff; passwords and CAPTCHAs need the owner |
| The debugger bar | – | – | Owner-verified probe (`PLATFORM_BROWSER.md` §2.1) | While FERB is attached to a tab of one of your profiles, that profile's visible windows show "FERB started debugging this browser"; FERB's own minimized window shows nothing; pressing Cancel makes the act in flight unknown and the task re-reads |

## FERB's own identity

| Capability | Example request | Needs | Verified | Known limits |
|---|---|---|---|---|
| Websites by default | `play some lofi on youtube` | FERB's browser | Offline test (`test_supervisor`, `test_whose_browser`) | Uses FERB's accounts; say "my …" for yours |
| Browse as FERB | `using your own account, open youtube.com` | FERB's browser | Real test (`test_ferb_identity_real` part 2, owner-approved) | Never your accounts or browsers |
| Sign in with FERB's password and inbox code | `sign in to <site> with your account` | FERB's browser, the password in the Keychain | Real test (`test_ferb_identity_real` part 1: lookalike host, scheme and cross-origin frame refused; sealed values never on the wire) | Passwords and codes are sealed; the model never sees them |
| Email from FERB's address | `email me the summary` | FERB's browser (Gmail) | Real test (legacy safety suite at 0.1.0: send confirmed from the Sent folder) | Unnamed recipients require approval; 10 s undo; text only, no attachments; "me" asks your address once |

## Voice (push-to-talk)

| Capability | Example request | Needs | Verified | Known limits |
|---|---|---|---|---|
| Hold Right Option, speak, let go | "what's the time?" | `ferb setup --voice` | Owner-verified run (0.9.0, daily use) + offline (`test_push_to_talk`) | Holds under 0.3 s and Right Option + another key are dropped |
| On-device speech recognition | any request | – | Owner-verified run (0.9.0) | English; FERB's name is often misheard (the key is the trigger) |
| Spoken replies | any voice request | – | Owner-verified run (0.9.0) | Pressing the key cuts FERB off; `[voice] sounds = false` silences the chimes |
| Never a Bluetooth headset's mic | default input is AirPods | – | Offline test (`test_push_to_talk`) | The built-in mic is used instead |
| Answer FERB by voice | "yes" / "the second one" / "stop" | – | Offline test | The oldest waiting question; "stop" cancels a countdown |
| `ferb voice`, `ferb voice on|off` | – | – | Offline test | Off quits FERB.app and stops speech |

## Shell and files

| Capability | Example request | Needs | Verified | Known limits |
|---|---|---|---|---|
| Files in FERB's workspace | `in your workspace create notes.md with 'one'` | – | Real test (`test_schedules_real` writes a file) | – |
| Your folders | `read ~/Documents/plan.txt and summarize it` | Folder | Offline test (`test_fs`, `test_schedules`) | Asked once per folder you name; "Desktop/…" means yours; Finder names with "/" are found by their ":" name |
| Drive a terminal program | `use claude in terminal in my projects folder to do X` | – | Owner-verified run (0.12.0: Claude Code started, instructed and read) | Runs in FERB's shell, not your Terminal; output read as text, paged from the start |
| Move to Trash | `move old.txt in your workspace to the Trash` | – | Offline test (`test_fs`) | Trash, never permanent deletion |
| Shell commands | `how big is my Downloads folder?` | – | Real test (`test_ux_real`) + offline (`test_mac_shell`, real PTYs) | Starts in the workspace; FERB's files, the Keychain and protected paths refused; Homebrew and user bins on PATH |
| Background processes | `start a local web server in your workspace` | – | Offline test | `ferb ps` / `ferb kill`; ended with the task unless long-lived |
| Permanent deletion | `rm -rf build/` | – | Offline test (`test_mac_shell`) | Always asks, every time |
| After untrusted content | `clone <repo> and run its tests` | – | Offline test (`test_acceptance_one_worker` 7) | A network-fetching command (or a web page, email, message, download) exposes the worker: it loses shell, AppleScript and file-write for the rest of the run. `docs/OWNER_ACCEPTANCE.md` E expects the shell to survive a fetch; the code doesn't do that yet |

## Monitoring (the activity trail)

| Capability | Example request | Needs | Verified | Known limits |
|---|---|---|---|---|
| App focus and window titles | (automatic) | Mac | Real test (`test_sense_real`) | Metadata only; titles redacted; `[sense] window_titles = false` turns titles off |
| File events in chosen folders | (automatic) | `[sense] watch_folders` | Real test (`test_sense_real`) | Only listed folders; metadata only |
| Browser tabs | (automatic) | Paired | Offline test (`test_ext_bridge`, fake extension) | Incognito and `exclude_domains` never recorded |
| Idle / active, app installs | (automatic) | Mac | Offline test (`test_sense`) | – |
| Pause everything | `ferb pause` / `ferb resume` | – | Real test (`test_sense_real`) | Survives restarts; no real input while paused |
| Ask about your activity | `what was I working on yesterday afternoon?` | – | Offline test (`test_memory`, `trail.query`) | Reading browser and window rows marks the worker as having read untrusted text |
| Daily summaries | `ferb timeline` | – | Real test (`test_memory_real`, fixture day) | Written hourly for past days by code, before any model; raw rows kept 30 days |

## Time and schedules

| Capability | Example request | Needs | Verified | Known limits |
|---|---|---|---|---|
| Reminders | `remind me next friday at 5 pm to call mom` | – | Real test (`test_time_real`) | Code computes the date from the model's structured "when" and shows it |
| Correct a time word | `by next friday I meant this friday` | – | Offline test (`test_time`) | Your meaning is remembered |
| Timers | `set a timer for 20 minutes` | – | Offline test | – |
| Durable, delivered once | a reminder across a restart or sleep | – | Real test (`test_time_real`: crash → restart → exactly once) | Late delivery is marked overdue |
| One-off scheduled task | `in 10 minutes, write done.md in your workspace` | whatever the task needs | Real test (`test_schedules_real`) | Runs unattended; the answer waits for your next chat |
| Recurring task | `every morning at 9, summarize ~/Documents/inbox` | whatever the task needs | Real test (`test_schedules_real`) | Once, daily, weekdays, weekly, or every N ≥ 15 minutes; no monthly |
| Schedules held to your words | a run that would need an app you didn't name | – | Offline test (`test_schedules`) | Targets recorded when scheduled; a run wanting more doesn't run; questions can't be asked, approvals wait 1 h |
| Manage schedules | `cancel the inbox schedule` / `ferb schedules` | – | Real test (`test_schedules_real`) | Three failures in a row pause a schedule |
| Missed runs | Mac asleep at 9:00 | – | Offline test | One late run on wake; skipped if later than half the interval (max 12 h) |

## Memory

| Capability | Example request | Needs | Verified | Known limits |
|---|---|---|---|---|
| Remember what you say | `remember that my dentist is Dr. Rao` | – | Real test (`test_memory_real`) | Only from your own words (the model's quote is checked), never pasted text |
| Manage memory | `ferb memory list / show / add / edit / retract` | – | Offline test (`test_memory`) | Edits keep history |
| Forget completely | `ferb memory forget <id>` | – | Offline test (`test_memory`, 21.17) | Purges sources, index and export; summaries rebuilt or deleted |
| Learn from your activity | (daily) | – | Real test (`test_memory_real`) | Observed habits activate after 3 separate days; inferred items wait for `ferb memory confirm`; FERB's own actions never count |
| Use memory in a turn | a request that depends on a remembered fact | – | Offline test | Keyword (FTS5) recall, no embeddings |
| Procedure memory (how a task was done) | – | – | Not built | Rework phase 5; `docs/REWORK_PLAN.md` |

## Safety (what you'll notice)

| Behaviour | How to see it | Verified | Notes |
|---|---|---|---|
| Sends to people you didn't name ask first | the worker wants to reply to someone | Offline test (`test_gateway`, `test_acceptance_one_worker` 4) | Once per request, effect and target, with the worker's reason; your own message naming the target is the approval |
| The floor raises a too-low declaration | a worker presses "Send" declaring `navigate` | Offline test (`test_acceptance_one_worker` 5) | One floor word list: Send / Submit / Call / Pay / Buy / Order / Delete / Remove / Unsubscribe / Sign out; Return in a message box is a submit |
| Undo countdown for sends and calls | `✉️ Sending to … in 10 s… (Ctrl-C to cancel)` | Real test (`test_contacts_real`) | `ferb undo` also cancels |
| Pay / destroy always ask | a payment, `rm -rf`, a password change | Offline test (`test_acceptance_one_worker` 6, `test_ux`) | Even inside a request that asked for it; a no is reported as failed |
| Untrusted content can't steer the worker | a page containing instructions | Offline test (`test_policy`, `test_acceptance_one_worker` 7) | Exposure removes shell / AppleScript / file-write; owner data may only go to a target of the request, something you named or approved |
| Password fields and FERB's secrets protected | any sign-in page | Real test (`test_ferb_identity_real`) | – |
| Emergency stop | `ferb stop` / `ferb resume-tasks` | Offline test (`test_stop`, `test_gateway` 21.2) | Survives restarts; a revoked token can't call a tool within a second |
| Runaway guards | a stuck task | Offline test | Same failing call 3×, loops, 4 h / 50 worker runs / 1,000 tool calls; a worker silent for 180 s is cancelled |
| Jev second opinion (shadow) | `ferb decide stats` | Real test (`test_decide_real`) | Only stricter; off until you `ferb decide enable` a question |
| The legacy §21 safety suite (current ARCHITECTURE.md §14) | `pytest` / `pytest -m real` | Real at 0.1.0 (all 24); offline since the 0.17.0 rework (`test_policy`, `test_gateway` and the acceptance set) | A real re-run after the rework isn't recorded |

## CLI

| Command | Verified | Notes |
|---|---|---|
| `ferb setup`, `setup --update`, `doctor` | Owner-verified run (daily use); offline (`test_setup_flow`, `test_install`, `test_launchd_real` real) | Permissions must go to FERB's interpreter |
| `ferb setup --identity`, `--jev`, `--voice`, `--codex-login` | Owner-verified run (identity, Jev, voice); offline | – |
| `ferb chat` | Real test (`test_ux_real`) | – |
| `ferb trace`, `logs`, `timeline` | Offline test (`test_cli`) | – |
| `ferb watch` | Owner-verified run (three runs, 2026-10-02) + offline (`test_rework_fixes`) | Mac steps on TextEdit and Calculator; browser steps in a throwaway Chrome; never your profiles |
| `ferb stats` | Offline test (`test_rework_fixes`) | Reads the database directly; no daemon needed |
| `ferb browsers`, `browsers pair` / `unpair` | Owner-verified run (spare and real profiles) + offline | `[gates] owner_profiles` changes guidance only; it does not enforce pairing |
| `ferb memory …`, `reminders`, `schedules`, `device`, `autonomy`, `undo`, `stop`, `resume-tasks`, `pause`, `resume`, `ps`, `kill`, `decide …` | Offline test | – |
| `ferb uninstall [--purge]` | Offline test (`test_install`) | Doesn't remove the Jev key or macOS permission entries |
| `python -m ferb.act.mac.probe <bundle> [--seed]` | Owner-verified run (`PLATFORM_MACOS.md` §13) | TextEdit, Calculator and Finder only; never quits an app |

## What FERB can't do yet

- **Other browsers:** Safari and Firefox aren't supported (read/navigate only would be possible; see
  `docs/PLATFORM_BROWSER.md` §8–9). A browser without the extension can be read through Accessibility but not acted on.
- **Signing in as you**, CAPTCHAs, purchases of its own: a signed-out page or a human check stops the task; any
  payment step asks and none has been tested.
- **Telephony:** no calls outside apps.
- **Email attachments** from FERB's address (uploads into web forms work).
- **Monthly schedules**; tasks over 4 hours, 50 worker runs or 1,000 tool calls.
- **Pixel clicks in the background** reach only windows on the current desktop; occluded same-desktop windows and
  Electron apps haven't been probed with the new driver (`docs/PLATFORM_MACOS.md` §13.6).
- **Learning by doing** beyond the per-app mechanism profile: procedure memory isn't built.
- **Learned trust:** FERB doesn't offer to stop asking about an action type; edit `[policy] trusted_kinds`.
- **Other platforms, other users, a GUI, sync across devices**: no.
- **Arbitrary code isolation:** the shell runs as you, inside FERB's rules; no sandbox per resource.

0.20.1: offline regressions cover default-profile authority, corrected legacy retries, native-browser bypass refusal,
disconnected tab diagnostics and no visible wake page after connection timeout. Live extension recovery remains
owner-operated; no new background/no-Space-shift claim is made.

0.21.0: bounded native scrolling and optional window-targeted input are implemented; owner verified WhatsApp wheel scrolling; broader gesture/app verification remains pending. See `docs/BACKGROUND_SCROLL_021.md`.
