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
| Pasted text is data, not instructions | paste an email, then `summarize this` | – | Offline | Targets (recipients, sites) must be in your own words, not the paste |
| Inline approvals | a request in `careful` mode | – | Verified (one question per target and action type) | Unanswered questions expire after 10 min (1 h for scheduled runs) |
| Plain wording, `--verbose` for internals | `ferb chat --verbose` | – | Offline | – |

## Mac

| Capability | Example request | Needs | Status | Known limits |
|---|---|---|---|---|
| List apps, read a window | `what's open in Finder right now?` | Mac | Verified (Finder snapshot) | Password fields are never read |
| Operate an app through Accessibility | `in TextEdit, type 'hello' in the open document` | Mac | Offline (real input, driver level: verified) | Falls back to real mouse/keyboard, which pauses while you use yours |
| AppleScript inside one scriptable app | `make a note in Notes titled 'groceries' with milk and eggs` | Mac, Automation for that app (asked once) | Offline (planning the Notes route: verified) | One app per script; no System Events, shell or nested `tell` |
| Open an app | `open Calculator` | Mac | Offline | Apps on `[privacy] exclude_apps` are refused |
| Screenshot one window, click by position | `in <app>, click the blue Export button` | Mac, Screen | Verified (probe window: window-only capture, clicks within 2 pt; reading an image: Codex 4/5) | Windows showing a password field aren't captured; excluded apps refused; images never saved |
| Windows on other Spaces / full screen | an app in full screen on another Space | Mac, Screen | Offline | – |
| Yield to you | move the mouse while FERB clicks | Mac (Input Monitoring if needed) | Verified (marked vs unmarked input told apart) | FERB waits, then continues or says where it stopped |
| Pick the app for a named service | `video call Rahul on WhatsApp` | the app installed | Verified (route chosen and shown; planning only) | Uses your last 14 days of activity; your correction wins |
| Calls and messages in Mac apps | `message Priya on WhatsApp: running late` | Mac | Offline | Contact must be on screen; several matches → "Which Priya?"; 10 s undo countdown |

## Browser (your paired profiles)

FERB uses your browser only when your words point at it: "my browser", "my Brave", "in Brave", "my Chrome", "my
<profile> profile", "this / the current tab", or "my <service>" ("my LinkedIn"). A plain "do X on YouTube" uses
FERB's own browser (next section).

| Capability | Example request | Needs | Status | Known limits |
|---|---|---|---|---|
| Open and read pages in your signed-in profile | `open <site> in my <profile> profile and tell me what's on it` | Paired | Verified (local test pages, spare profile) | Tabs open in the background and never take focus |
| Click, type, select (scripted or trusted input) | `search for 'standing desk' on <site> in my <profile> profile` | Paired | Verified (local pages: scripted and `isTrusted` input) | Password/code fields are never typed into |
| Choose the profile | `check my email` (site open in one profile) | Paired | Offline | Order: open tab → your trail → default browser → ask |
| Your profile when you say so | `in my <profile> profile, play some lofi on youtube` | Paired | Offline (real run pending) | Only paired profiles are ever routes |
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

## Shell and files

| Capability | Example request | Needs | Status | Known limits |
|---|---|---|---|---|
| Files in FERB's workspace | `in your workspace create notes.md with 'one', then append 'two'` | – | Verified | – |
| Your folders | `read ~/Documents/plan.txt and summarize it` | Folder | Offline | Asked once per folder you name; content FERB reads can't trigger the question |
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
| Unrequested actions blocked | ask for a summary; FERB never sends | Offline (runtime lockdown verified) | Intent is fixed from your words at plan time |
| Undo countdown for sends and calls | `📞 Calling … in 10 s… (Ctrl-C to cancel)` | Verified | `ferb undo` also cancels |
| Irreversible actions always ask | a payment, `rm -rf`, a password change | Offline | Even inside a request that asked for it |
| One approval per target and type | `careful` mode, three edits to one file | Verified | A different target is a new question |
| Exploring with a stated reason; route change asks | "Your Brave had no call button. The WhatsApp app does. Call Rahul from there?" | Offline | A yes adds that app/site for the rest of the request |
| Untrusted content can't steer actors | a page containing instructions | Verified (reader → actor, typed fields) | Reduced, not eliminated |
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
| `ferb logs`, `trace`, `timeline` | Offline (used throughout development) | – |
| `ferb browsers`, `browsers pair` / `unpair` | Verified (pairing a spare and a real profile) | Needs `[gates] owner_profiles = true` for real profiles |
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
- **Other platforms:** no GUI, voice, phone or web access to FERB, and no sync across devices.
- **Other users:** FERB is for the one macOS user who installed it.
- **Arbitrary code isolation:** the shell runs as you, inside FERB's rules. There's no sandbox per resource.
