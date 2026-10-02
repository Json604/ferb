# What FERB can do (v0.1)

This is a checklist for trying FERB out. Each row gives an example request, what it needs, and how well it's tested.

**Status**
- **Verified**: passed in a real run (real model, and a real browser or Mac where relevant). The note says what was
  run. Several real runs used local test pages or a spare browser profile, not your accounts.
- **Offline**: covered by the offline test suite (fakes for models, browsers and the Mac), not yet run for real.
- **Untested**: should work from how it's built, but no test covers it.

**Needs**
- **Mac** = the Accessibility permission granted to FERB's interpreter.
- **Screen** = Screen Recording.
- **Paired** = a paired, open Chrome/Brave profile with the FERB extension.
- **FERB's browser** = FERB's own Chrome profile set up (`ferb setup --identity`), paired and open.
- **Folder** = you said yes to FERB using that folder (asked the first time).

If something a request needs is missing, FERB says so ("I can't do that right now. Unavailable: …") and tells you how
to fix it.

## Chat

| Capability | Example request | Needs | Status | Known limits |
|---|---|---|---|---|
| Talk, answer, introduce itself | `hi, who are you and what can you do?` | – | Verified (session replay) | Plain conversation never starts workers |
| Answer from analysis of your Mac | `what apps take the most space on my mac, and how can I free some up?` | Mac, shell | Verified (session replay) | The planner may want screenshots too; it says so if Screen is missing |
| Ask one question when unclear | `send it to him` | – | Offline | One question at a time |
| Continuity across chats and restarts | `what were you doing earlier? did you finish?` | – | Verified (stop + new chat) | Sees the last day of conversation and tasks |
| Tasks outlive the chat window | close chat mid-task, reopen later | – | Offline | Finished answers show once; a macOS notification goes out when chat is closed |
| Stop a task | Ctrl-C while it works | – | Verified (stop through the same call) | A send held for undo is cancelled too |
| Pasted text is data, not instructions | paste an email, then `summarize this` | – | Offline | Targets (recipients, sites) must be in your own words, not the paste; a whole-message paste (or "hey" + a paste) asks whether it's your request; `[policy] pasted_text_is_data = false` turns the rule off |
| One worker per request, gated at the action | `search WhatsApp for the syllabus sheet and tell me the gym timing` | – | Offline (acceptance set replays the run history) | The worker has every tool its identity allows; each act call declares an effect (navigate / edit / send / pay / destroy) that FERB raises to the control's floor |
| A worker's question in chat | `sign in to my account` (two accounts on screen) | – | Offline | `owner_ask`: numbered options or your own words; Enter skips; never during a scheduled run |
| See what a task did | `ferb trace` / `ferb trace a1b2c3d4` | – | Verified (rendered a real run, including a driven Claude Code session) | Latest task by default; `--full` shows everything |
| Speak replies | `ferb chat --speak` | – | Verified (owner use) | The Mac's built-in voice (`say`) |
| Inline approvals | a request in `careful` mode | – | Verified (one question per target and action type) | Unanswered questions expire after 10 min (1 h for scheduled runs) |
| Plain wording, `--verbose` for internals | `ferb chat --verbose` | – | Offline | – |

## Mac

| Capability | Example request | Needs | Status | Known limits |
|---|---|---|---|---|
| List apps, read a window | `what's open in Finder right now?` | Mac | Verified (Finder snapshot) | Password fields are never read |
| Read what's really there | (automatic) | Mac | Verified (TextEdit: text area, close button, save sheet's Delete / Cancel / Save all listed; Calculator's buttons by description) | A control is anything that exposes an action (pressable rows, groups, cells, images included); roles, actions, disabled / selected / expanded state, headings; an open sheet, dialog, pop-up menu or popover is in the snapshot with its place; the app's windows are listed and one can be read by title or id |
| See an app window with its controls numbered | (automatic in app work) | Mac, Screen | Offline (mark placement verified on a real window) | Not for browsers yet; no image for excluded apps or password fields |
| Read an app wherever it is | `open whatsapp and tell what is the topmost chat message` | Mac | Verified (owner's run: WhatsApp read on another desktop, no switch) | Chat rows include their latest message preview |
| Operate an app from the background | `in TextEdit, type 'hello' in the open document` | Mac | Verified (`ferb watch`, owner watching: 0 of 25 steps moved anything; real test: new document by menu, typed, edited, scrolled, Calculator 7 × 6 = 42, frontmost app polled every 50 ms never changed) | Per primitive, in this order: press = Accessibility press → the enclosing control → focus + Return; type = keystrokes to the app's process (read back) → Accessibility keyboard (apps it worked in) → the box's value; edit = exact range through the text's selection (never arrow keys); shortcut = its menu item, refused while the app reports it disabled; escape = the open sheet / dialog / menu cancelled, never blind; scroll = Page Up / Down keys with the area focused → scroll bar; pop-ups by pressing the button then the item; types only when the box really has focus (a dialog can hold it) |
| Learn what works per app | (automatic) | Mac | Offline (seeding from `ferb.act.mac.probe --seed` verified on TextEdit / Calculator) | Each mechanism's landed / failed counts per app in `~/.ferb/device/apps/<bundle>.json`; three misses in a row skip it until a later success; nothing per app is written by hand |
| Foreground only by leave | an app that ignores every background way | Mac (Input Monitoring if needed) | Offline | The result says what failed; only a call with `allow_foreground=true` brings the app in front: one switch per stretch of work; your first key/click held back so it never lands in FERB's app; your app back after 3 s idle |
| Electron apps (Slack, Discord, VS Code) | `open slack and check my messages` | Mac | Offline | FERB asks each for its full Accessibility tree; waits for the main window after opening |
| AppleScript inside one scriptable app | `make a note in Notes titled 'groceries' with milk and eggs` | Mac, Automation for that app (asked once) | Offline (planning the Notes route: verified) | One app per script; no System Events, shell or nested `tell` |
| Open an app | `open Calculator` | Mac | Verified (WhatsApp: opened, window reopened from its own menu, read; your app stayed in front) | Background launch; read-only steps may open apps to look; apps on `[privacy] exclude_apps` are refused |
| Screenshot one window, click by position | `in <app>, click the blue Export button` | Mac, Screen | Verified (probe window: window-only capture, clicks within 2 pt; reading an image: Codex 4/5) | The point is first turned into the control under it (the app's own hit test) and pressed through Accessibility; a click posted to the app alone only for a window on the current desktop, kept if something changed; the real mouse only with `allow_foreground=true`. Windows showing a password field aren't captured; excluded apps refused; images never saved |
| Windows on other Spaces / full screen | an app in full screen on another Space | Mac, Screen | Offline | – |
| Yield to you | move the mouse while FERB clicks | Mac (Input Monitoring if needed) | Verified (marked vs unmarked input told apart) | Only matters in the foreground; FERB puts your app back, waits until you stop, then continues or says where it stopped |
| Pick the app or the site for a named service | `video call Rahul on WhatsApp` | the app installed | Offline | The model decides from an environment line (what's installed, paired and running); nothing in code picks a route |
| Calls and messages in Mac apps | `message Priya on WhatsApp: running late` | Mac | Offline | Contact must be on screen; several matches → "Which Priya?"; Send button, never Return; 10 s undo countdown; a second message to the same person asks |
| No arrow-key editing, no app switching, no quitting | (automatic) | Mac | Offline (⌘A beep and arrow beep heard by the owner, 2026-10-02) | Text is edited by range or replaced whole; ⌘Tab, Control+arrows, ⌘Space, ⌘Q / ⌘H / ⌘M and AppleScript `activate` are refused |

## Browser (your paired profiles)

FERB uses your browser when the request is about your apps, accounts or profiles ("my Gmail", "in Brave", "my
school gmail in chrome", "the current tab"): the model picks identity `owner`; a plain "do X on YouTube" is a
`ferb` task in FERB's own browser (next section). The code floor: a `ferb` task never gets your browsers, Mac or
folders. The browser or profile you name is the only one used.

| Capability | Example request | Needs | Status | Known limits |
|---|---|---|---|---|
| Open and read pages in your signed-in profile | `open <site> in my <profile> profile and tell me what's on it` | Paired | Verified (local test pages, spare profile) | Tabs open in the background |
| Click, type, select (scripted or trusted input) | `search for 'standing desk' on <site> in my <profile> profile` | Paired | Verified (local pages: scripted and `isTrusted` input) | Password/code fields are never typed into |
| Choose the profile | `check my email` (site open in one profile) | Paired | Offline | Order: open tab → your trail → default browser → ask |
| The profile you name, and only that one | `open my school gmail in chrome and find the latest assignment email` | Paired | Verified (owner's run: the named Chrome profile's Gmail searched, the email and its form opened) | Not connected → FERB says why and how (`ferb browsers pair`), never another browser |
| Install + pair the extension in a profile | `ferb browsers pair school` | Chrome/Brave | Offline (the profile was paired by hand before the guide existed) | Chrome allows only Load unpacked by hand; FERB opens the page, copies the path, shows the clicks, waits, pairs |
| Wake a browser without moving you | any browser task with Chrome closed or open | Paired | Verified (owner watching, with an event detector: windowless wake; owner's search run) | A first window is created minimized; if a browser still jumps forward, your app comes back |
| Your profile when you say so | `in my <profile> profile, play some lofi on youtube` | Paired | Offline (real run pending) | Only paired profiles are ever used |
| Use a tab you already have open | `in my brave, keep scrolling the comments in the current youtube tab` | Paired | Offline (real run pending) | FERB lets go of the tab afterwards and never closes it; scrolling a changing page isn't treated as a loop |
| Account check before consequential actions | a send on a site where two accounts are signed in | Paired | Offline (extractors verified on FERB's own Gmail/YouTube) | Extractors exist for Gmail and YouTube; other sites ask once per site + profile |
| Calls in web chat, contacts checked | `video call Rahul` on a chat page | Paired | Verified (local fake chat page; 8/10 over the final 5 runs) | The worker sometimes stops to ask you itself instead of letting FERB ask (safe, not a guess) |
| Account and security pages always ask | `turn on 2FA in <site> settings` | Paired | Offline | Every action there asks, every time |
| Read your web email in a profile | `what are my unread emails in Gmail?` | Paired | Untested | A signed-out page stops the task; FERB never signs in as you |

## FERB's own identity

| Capability | Example request | Needs | Status | Known limits |
|---|---|---|---|---|
| Websites by default | `play some lofi on youtube` / `post this on linkedin` | FERB's browser | Offline (real run pending) | Uses FERB's accounts; say "my …" for yours |
| Browse as FERB | `using your own account, open youtube.com and check you're signed in` | FERB's browser | Verified (session held through the extension) | Never your accounts or browsers |
| Sign in with FERB's password and inbox code | `sign in to <site> with your account` | FERB's browser, the password in the Keychain | Verified (owner-assisted one-time code) | Passwords and codes are sealed; the model never sees them; lookalike or cross-origin frames refused |
| Email from FERB's address | `email me the summary` | FERB's browser (Gmail) | Verified (3/3 through Gmail's compose page) | Only to recipients you named; 10 s undo; text only, no attachments; "me" asks your address once |

## Voice (push-to-talk)

| Capability | Example request | Needs | Status | Known limits |
|---|---|---|---|---|
| Hold Right Option, speak, let go | "what's the time?" | `ferb setup --voice` (Microphone, Speech Recognition) | Verified (owner use) | Taps under 0.3 s and Right Option + another key are dropped; start sound only on a real hold |
| On-device speech recognition | any request | – | Verified (owner's voice: sentence exact, text ready 0.02 s after release) | English; FERB's name is often misheard (the key is the trigger) |
| Spoken replies | any voice request | – | Verified (owner use) | Pressing the key cuts FERB off; `[voice] sounds = false` silences the start/stop sounds |
| Never a Bluetooth headset's mic | default input is AirPods | – | Offline (and spike: never probed) | Uses the built-in mic instead |
| Answer FERB by voice | "yes" / "the second one" / "stop" | – | Offline | Answers the oldest waiting question; "stop" cancels a send/call countdown |
| See it live, turn it off | `ferb voice` / `ferb voice off` / `ferb voice on` | – | Offline | Off quits FERB.app and stops speech |

## Shell and files

| Capability | Example request | Needs | Status | Known limits |
|---|---|---|---|---|
| Files in FERB's workspace | `in your workspace create notes.md with 'one', then append 'two'` | – | Verified | – |
| Your folders | `read ~/Documents/plan.txt and summarize it` | Folder | Offline | Asked once per folder you name; content FERB reads can't trigger the question; "Desktop/…", "Documents/…" mean yours; Finder names with "/" are found by their real ":" name |
| Drive a terminal program | `use claude in terminal in my projects/notes folder to do X` | – | Verified (owner's run: Claude Code started, instructed and read) | Interactive programs run in FERB's shell (not your Terminal window); FERB reads their screen as text |
| Move to Trash | `move old.txt in your workspace to the Trash` | – | Offline | Trash, not permanent deletion |
| Shell commands | `how big is my Downloads folder?` | – | Offline (available in the verified storage replay) | Starts in the workspace; FERB's files, the Keychain and protected paths refused; lost after reading untrusted content |
| Background processes | `start a local web server in your workspace` | – | Offline | `ferb ps` / `ferb kill`; end with the task unless long-lived |
| Permanent deletion | `rm -rf build/` | – | Offline | Always asks, every time |

## Monitoring (the activity trail)

| Capability | Example request | Needs | Status | Known limits |
|---|---|---|---|---|
| Record app focus and window titles | (automatic) | Mac | Verified | Metadata only; titles redacted; `[sense] window_titles = false` turns titles off |
| Record file events in chosen folders | (automatic) | `[sense] watch_folders` | Verified | Only listed folders; metadata only |
| Record browser tabs | (automatic) | Paired | Offline | Incognito and `exclude_domains` never recorded |
| Idle / active, app installs | (automatic) | Mac | Offline | – |
| Pause everything | `ferb pause` / `ferb resume` | – | Verified | Survives restarts |
| Ask about your activity | `what was I working on yesterday afternoon?` | – | Offline | Reading browser and window rows marks the worker as having read untrusted text |
| Daily summaries | `ferb timeline` | – | Verified (distillation of a fixture day) | Written hourly for past days; raw rows kept 30 days, summaries forever |

## Time and schedules

| Capability | Example request | Needs | Status | Known limits |
|---|---|---|---|---|
| Reminders | `remind me next friday at 5 pm to call mom` | – | Verified (computed date shown) | FERB picks the date in code and shows it; a reminder only notifies |
| Correct a time word | `no, by next friday I meant this friday` | – | Verified (reminder moved, meaning remembered) | Your meaning is used from then on |
| Timers | `set a timer for 20 minutes` | – | Offline | – |
| Durable, delivered once | a reminder across a restart or sleep | – | Verified (crash → restart → exactly once) | Late delivery after sleep is marked overdue |
| One-off scheduled task | `in 10 minutes, write done.md in your workspace containing ok` | whatever the task needs | Verified | Runs unattended; the answer waits for your next chat |
| Recurring task | `every morning at 9, summarize the files in ~/Documents/inbox` | whatever the task needs | Verified (shortened interval; cancelled from chat) | Once, daily, weekdays, weekly, or every N ≥ 15 minutes; no monthly |
| Manage schedules | `cancel the inbox schedule` / `ferb schedules` | – | Verified (cancel from chat) | Three failures in a row pause a schedule |
| Missed runs | Mac asleep at 9:00 | – | Offline | One late run on wake; skipped if later than half the interval (max 12 h) |
| Unattended approvals | a scheduled step that needs your OK | – | Offline | Notification, wait 1 h, then skip and report |
| Time awareness | chat after a few hours away | – | Offline | "Since we last talked" after 1 h; non-urgent notifications held while you're idle |

## Memory

| Capability | Example request | Needs | Status | Known limits |
|---|---|---|---|---|
| Remember what you say | `remember that my dentist is Dr. Rao at Smile Clinic` | – | Verified | Only from your own words (a quote is checked), never pasted text |
| Manage memory | `ferb memory list / show / add / edit / retract` | – | Offline | Edits keep history |
| Forget completely | `ferb memory forget <id>` | – | Offline | Purges sources, index and export; summaries rebuilt or deleted |
| Learn from your activity | (daily) | – | Verified (fixture day) | Observed habits activate after 3 separate days; inferred items wait for `ferb memory confirm`; FERB's own actions never count as your habits |
| Route and contact preferences | `use the web version for WhatsApp` / `Rahul always means Rahul Sharma` | – | Offline | Only lasting when you state them |
| Use memory in planning | a request that depends on a remembered fact | – | Offline | Keyword search (no embeddings yet) |

## Safety (what you'll notice)

| Behavior | How to see it | Status | Notes |
|---|---|---|---|
| Sends to people you didn't name ask first | ask for a summary; the worker wants to reply to someone | Offline (acceptance set) | Navigate and edit run; a send asks once per target with the worker's reason unless your own message names the target (pasted text never counts) |
| Undo countdown for sends and calls | `📞 Calling … in 10 s… (Ctrl-C to cancel)` | Verified | `ferb undo` also cancels |
| Irreversible actions always ask | a payment, `rm -rf`, a password change, a "Delete" button | Offline | Even inside a request that asked for it; a control whose label says Send / Pay / Delete can't be declared harmless |
| One approval per target and effect | a send to one unnamed person, three messages | Offline | A different target is a new question |
| Untrusted content can't steer the worker | a page containing instructions | Offline | Owner data read in the request may only go to a target of it, something you named, or something you approved; a worker that read a web page loses shell, AppleScript and file-write |
| Password fields and FERB's secrets protected | any sign-in page | Verified (web password check, sealed secrets) | – |
| Emergency stop | `ferb stop` / `ferb resume-tasks` | Offline | Survives restarts |
| Runaway guards | a stuck task | Offline | Same failing action 3×, loops, 4 h / 50 workers / 1,000 tool calls |
| Jev second opinion (shadow) | `ferb decide stats` | Verified (one batched real request) | Only stricter; off until you `ferb decide enable` a question |

## CLI

| Command | Status | Notes |
|---|---|---|
| `ferb setup`, `setup --update`, `doctor` | Verified (on the development Mac) | Permissions must go to FERB's interpreter |
| `ferb setup --identity` | Verified | Refuses a profile that holds any account besides FERB's |
| `ferb setup --jev` | Verified | Key in the Keychain |
| `ferb chat` | Verified | – |
| `ferb logs`, `timeline` | Offline (used throughout development) | – |
| `ferb trace [task]` | Verified (real runs) | Latest task by default; every step and tool call |
| `ferb setup --voice`, `ferb voice [on\|off]` | Verified (setup, use); on/off offline | Needs the "FERB Local Code Signing" certificate |
| `ferb setup --codex-login` | Offline | FERB's own Codex sign-in, apart from your personal Codex |
| `ferb browsers`, `browsers pair` / `unpair` | Verified (pairing a spare and real profiles) | Needs `[gates] owner_profiles = true` for real profiles; guides installing the extension where it's missing |
| `ferb memory …`, `reminders`, `schedules`, `device`, `autonomy`, `undo`, `stop`, `resume-tasks`, `pause`, `resume`, `ps`, `kill`, `decide …` | Offline | – |
| `ferb uninstall [--purge]` | Offline | Doesn't remove the Jev key or macOS permission entries |

## What FERB can't do yet

- **Other browsers:**
  - Safari and Firefox aren't supported.
  - A browser without the extension can be read through Accessibility and operated only by screenshot clicks, as a
    fallback. If the extension is installed there but unpaired, FERB asks you to pair it instead.
  - The extension isn't in a web store; it's loaded unpacked.
- **Signing in as you:** FERB never signs in to your accounts. A signed-out page stops the task.
- **CAPTCHAs and human checks:** there's no handling; the task stops.
- **Purchases:** there's no payment flow of its own. Any payment step always asks, and none has been tested.
- **Telephony:** FERB can't place phone calls outside apps (no Phone app route tested, no SIM or telephony
  integration).
- **Attachments:** FERB can't send email attachments or upload files to sites.
- **Monthly schedules** and other complex recurrence (e.g. "the first Monday of each month").
- **Tasks longer than 4 hours,** or more than 50 worker runs or 1,000 tool calls in one task.
- **Watching folders you haven't listed,** or recording keystroke content, clipboard or screen contents (by design).
- **Excluded apps:** FERB never acts on or captures apps on your exclusion list (by design).
- **Learned trust:** FERB doesn't offer to stop asking about an action type; edit `[policy] trusted_kinds` instead.
- **Planner lookups:** the planner doesn't look things up itself (e.g. "is app X installed?"); it plans from the
  summary it's given.
- **Other platforms:** no GUI, phone or web access to FERB, and no sync across devices. Voice is push-to-talk on the
  Mac only (no wake word, no always-on listening, no system-audio listening yet).
- **Browser focus on macOS:** a new browser window can still jump to the front briefly (a Chromium problem); FERB
  takes you back.
- **Other users:** FERB is for the one macOS user who installed it.
- **Arbitrary code isolation:** the shell runs as you, inside FERB's rules. There's no sandbox per resource.
