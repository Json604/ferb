# FERB — product feature reference

Prepared 2026-10-14 from the `astra_resume` branch (checkout version `0.27.0.dev1`), its plan of record, the owner
acceptance record and the launch-week work (statuses updated once every launch-week task was integrated). Written for
FERB itself, to plan the launch campaign around the public open-source release on **2026-10-21** (site:
https://ferb.kartikey.xyz, privacy page: https://ferb.kartikey.xyz/privacy; the repository goes public on launch day).

Every feature below carries a status. Marketing must never promise more than the status says.

## Status legend

| Label | Meaning |
|---|---|
| **Shipped** | In the code on `astra_resume` from the 0.27 milestones (F0–F10) or earlier. "Live-verified" is added only where `docs/OWNER_ACCEPTANCE.md` records the owner's live result; otherwise live acceptance is pending. |
| **Landing this week — integrated, not live-tested (Lnn)** | Launch-week work, reviewed and on `astra_resume` (every launch-week task is integrated; the whole-branch final review is still to come). Not yet live-tested by the owner unless stated. The owner's installed copy picks it up at the next `ferb setup --update`. |
| **After launch** | Decided or discussed, deliberately deferred. |
| **Known limit** | An honest residual or caveat. |

Where things stand overall:
- The owner signed off milestones F0–F4 live (2026-10-07 to 2026-10-09). F5–F10 passed full offline checkout
  validation (about 2,000+ tests; 2,262 at the last integration) and are installed; their formal owner live
  acceptance is still pending.
- The owner has used FERB daily through the notch app since the 2026-10-11 install. That use produced the launch-week
  fix list (L1–L28 in `docs/superpowers/specs/2026-10-13-launch-week-fixes-design.md`). All of it is now integrated on
  `astra_resume`; the final review, the owner's install and the live checks (`docs/OWNER_ACCEPTANCE.md`, "0.27 launch
  week") are pending.
- The installed copy now runs the Codex model `gpt-5.6-sol` (set in the owner's config on 2026-10-14).

---

## 1. What FERB is

FERB is an open-source personal assistant for macOS (Apple silicon). The owner asks in plain words, from a small
chat panel that lives in the MacBook's notch (or from a terminal), and FERB does the task the way a person would:
in the owner's Mac apps, in their browser profiles, in a sandboxed shell and in their files. It works **in the
background** while the owner keeps working: it leaves the cursor, keyboard focus and current Space alone and only
takes the foreground when the owner explicitly allows it. It **perceives what happens on the Mac all day** (apps,
windows, tabs, typed text per field, clipboard, notifications, new software), keeps it locally, and tells the owner
in chat when something matters. It **learns by doing**: every task becomes a permanent record, and repeated verified
tasks become reusable procedures. It is **secure by design**: known kinds of secrets are removed before anything
reaches an AI model, the shell runs in a macOS sandbox whose only internet route is a recording proxy, FERB's own file
changes are recoverable, every action is recorded, and code (never a model) asks the owner before money,
account-security changes, permanent deletion and sends to new people, when the step declares that effect. It
**works with the AI subscription the owner already has**: the Codex CLI (ChatGPT sign-in) or
Claude (Claude Agent SDK), or per-token API keys for Anthropic, OpenAI or Google.

Who it is for: Mac users who want a capable agent doing real work across their own apps and accounts (research,
email and messaging chores, forms, downloads, file and disk work, coding-agent supervision, reminders and schedules),
who care where their data goes and want to see exactly what the agent did.

What makes it different (each claim checked in code and docs):
- **Background-first Mac control.** Accessibility actions on the exact window, app-directed events, background
  browser tabs through its own extension, a sandboxed shell. Foreground input needs an explicit tool choice and the
  owner's direction (`ARCHITECTURE.md` §6). A desktop sentinel records every focus or Space change during a task.
- **Perceives 24/7, locally.** Passive watchers record redacted activity on the Mac; data stays on the Mac
  (`ferb/sense/`). Telling the owner about something (a "notice") needs the optional Jev service (see Perception).
- **Learns by doing.** Episodes, lessons and procedures with a "learned" status after three verified successes
  (`ferb/memory/`). Honest note: learning only started working reliably with the launch-week fix L16.
- **Secure by design.** Trust, record, recover (L1): a small set of code-enforced questions, everything recorded,
  FERB's own file changes recoverable. Secrets travel only through a sealed path.
- **Open source.** Apache-2.0 licensed; the repository goes public on 2026-10-21.
- **Your existing subscription.** Codex and Claude runtimes bill the owner's subscription; API runtimes bill per
  token (`ferb runtimes` labels each).

---

## 2. How a request flows, step by step

Each step names its status. File paths are given for traceability only.

1. **The owner sends a message** from the notch app (or `ferb chat`). Selected files and pasted text travel with it;
   pasted text is marked as outside data, never as the owner's own instructions. **Shipped.**
2. **Instant acknowledgement.** Before anything else, the daemon replies with a short neutral phrase ("Got it.",
   "On it.", …), picked at random from the owner-approved list and never one of the last eight used
   (`ferb/orchestrator/acks.py`). It claims nothing about what FERB will do. **Landing this week — integrated, not
   live-tested (L7).**
3. **The front window is captured at send.** App, window title, the browser's front tab URL/title (Safari, Chrome and
   its channels, Chromium, Brave, Edge, Vivaldi and Arc, once you've allowed FERB to automate that browser), and one
   redacted
   screenshot of that one window (FERB's own windows skipped). The planner sees it with the message and can answer a
   screen question directly from the picture; the worker for that request starts from the same picture.
   **Landing this week — integrated, not live-tested (L12).** Before it, the model got one start-of-task picture of
   the app a worker first worked on; screenshots still come only when the model asks.
4. **The planner decides.** It receives, in this order: the situation (current time, owner activity, front window),
   running tasks and open questions, relevant memory, today's matching activity, and as much conversation as fits.
   It either answers directly (a reply or a clarifying question) or writes a task: a goal, outcome conditions the
   owner would recognise, which account (the owner's or FERB's own), and optionally a multi-step plan.
   **Shipped**, with the new context order and the model's real context window **Landing this week — integrated,
   not live-tested (L28)**. (Before, a hidden 32,768-token cap meant the situation, activity and memory blocks were
   usually squeezed out by long chat history.)
5. **Plan line.** As soon as the planner returns a task, a one-line plan appears in chat, before any worker acts.
   **Landing this week — integrated, not live-tested (L7/L8).**
6. **Workers act through FERB's gateway.** One worker per plan step, with only the tools its identity allows. Every
   tool call goes through the gateway (`ferb/daemon/gateway.py`), which validates arguments, checks paths, locks the
   resource, records the action and its origin, redacts the result, and asks the owner when code requires it. Tools:
   Mac apps via Accessibility, the browser via FERB's extension, a sandboxed shell with a recording proxy, files with
   recovery copies, timers, memory and evidence readers. **Shipped.**
7. **Live progress line.** While a worker works, one line under the plan updates in place ("reading github.com",
   "looking at Brave", "writing notes.md") with elapsed time. Code builds the text from the tool and its target, never
   from model text, so a web page cannot speak through it. It disappears when the answer arrives. **Landing this week
   — integrated, not live-tested (L8)** in the notch app; the terminal chat does not show it yet (`ferb logs --tools`
   shows step lines).
8. **The checker verifies.** A separate final check judges every outcome condition plus one extra: "my report to you
   says truthfully what was done, who did each part and with what". The checker gets observation-only tools, enforced
   by code: it can look but not click, type, send or write. **Shipped, live-verified (F4).**
   It starts with every tool call the worker made and its result, so it reads the records only for what didn't fit
   or got cut. **Landing — integrated (fix wave 6b)**; not live-tested.
9. **The answer.** One of three results: **done** (every condition confirmed), **done, but not confirmed** (nothing
   wrong found, but not everything could be confirmed from the background; each unconfirmed item is listed), or
   **failed** (with the real reason). A short answer shows first; full details stay under Details. **Shipped.**
   Short answers written in the owner's words for unfinished tasks, no "Verified." prefix (a check mark instead),
   follow-up corrections that re-plan, and a code-built **"Sites contacted:"** line at the end of the full answer:
   **Landing this week — integrated, not live-tested (L11, L2).** The check mark shows in the app.
10. **Learning.** Every task is written as a permanent episode (request, steps, checks, outcome, origins, desktop
    changes). An idle-time consolidator turns verified episodes into lessons and procedures; identity suggestions go
    to the owner as a digest. **Shipped**; the bugs that made learning fail in practice are fixed **Landing this week —
    integrated, not live-tested (L16)**. Learning by intent: each task is labelled with the kind of task it was, and a
    kind of task is first learned from three verified runs of it, whatever tools they used **Landing this week —
    integrated, not live-tested (L29)**.

Things that can happen along the way:
- **Questions.** A worker's question goes to the planner first; only real what/who/which choices reach the owner.
  Code-owned safety questions start with "FERB:" and offer Yes/No (Enter = No). **Shipped.** Questions shown inline
  in the notch chat while typing stays enabled, and questions that never expire while their task waits (they used to
  expire after 600 s): **integrated, not live-tested (L3, L9)**.
- **Steering.** The owner can correct a running task by just replying; the correction reaches the same running worker
  without restarting it. **Shipped (2026-10-11).** "Steering never records a No" and free-text answers routed by
  question id, in the owner's own words: **integrated, not live-tested (L3)**.
- **Stop.** Escape / `/stop` / the app's Stop button stops a task; `ferb stop` is a persistent emergency stop.
  **Shipped.**

---

## 3. Feature catalogue

### 3.1 The Mac app (notch UI, `ui/mac/FERBNotch`)

The app is the product (owner, 2026-10-14: the CLI was for testing). It talks to the same local daemon as the CLI
over an owner-only Unix socket (`docs/RPC.md`).

#### Notch panel, shortcuts and look
- What: a frosted-glass panel that lives in the hardware notch. Double-tap right Option opens it with the text field
  focused (and closes it); hover opens it; a pin keeps it open. It stays fixed to the notch across Space swipes and
  never steals focus for background updates.
- Owner sees: a compact composer ("Ask anything…"), a status dot (amber working, red attention, green done), two
  soft green glow pulses at the camera edge when work completes, low-volume sound cues (dictation start/stop,
  completion, attention; mute/volume in Settings, Cmd-comma).
- Status: **Shipped**, in daily owner use since 2026-10-11.

#### Continuous daily chat with history
- What: one ongoing conversation across days, Messages-style bubbles, Today plus History/date tabs; Markdown
  (tables, lists, links, code) rendered natively; remote images are not fetched automatically.
- Status: **Shipped.**

#### Instant acknowledgement, plan line and live progress line in chat
- What: the ack appears at once, the plan line follows when the planner returns, and one live line shows the current
  step with elapsed time until the answer arrives. Each event carries its request id, so two requests in flight never
  mix. A dropped AI service connection is said on that line the moment the service starts retrying (the Mac seems
  offline, or the service and its words), once per outage, and again when it reconnects; the task carries on.
- Status: **Landing this week — integrated, not live-tested (L6–L8; the connection line: fix wave 7).**

#### Late answers appear at the bottom as replies
- What: when an older request finishes after a newer one, its answer appears at the bottom quoting the original
  question, and the original turn says "answered below". Before, a late answer was written into its old spot, out of
  sight.
- Status: **Landing this week — integrated, not live-tested (L6).**

#### Verification check mark
- What: a small check mark on answers whose every condition was verified.
- Status: **Landing this week — integrated, not live-tested (L11)**, with the literal "Verified." text prefix removed.

#### Inline questions and hidden secret input
- What: FERB's questions show inline in the chat; typing stays enabled while one is open (before, a question
  replaced the chat). Secret prompts (passwords, codes) use a hidden field, never enter chat history and never take
  keyboard focus on their own. A new genuine question opens the panel once with amber attention.
- Status: **Landing this week — integrated, not live-tested (L9/L20).** Amber attention: **Shipped.**

#### Perception notices in chat
- What: when FERB notices something worth telling (see 3.3), it posts a FERB message with a small "noticed" tag.
  Any outside text it repeats is shown as a quote with its origin, never as FERB's own words. macOS banners are used
  only when no FERB client is connected.
- Status: **Landing this week — integrated, not live-tested (L9, L10).**

#### Learning digest buttons
- What: pending identity suggestions ("FERB thinks you prefer …") appear in chat with **Confirm** and **Not right**
  buttons. Nothing inferred becomes part of FERB's always-loaded picture of the owner without a Confirm.
- Status: **Landing this week — integrated, not live-tested.** Digest itself: **Shipped** (at most once a day).

#### Voice dictation
- What: hold right Command + Option to dictate; the transcript lands in the composer for review before sending.
  Uses Apple's speech recognition (on-device when the Mac supports it).
- Status: **Shipped**; a microphone-route crash was fixed 2026-10-11. Live microphone/device-switch acceptance
  pending.
- Limit: dictation is recognized on this Mac only, like push-to-talk and hearing (3.5): where on-device recognition
  isn't available for the language, it says so instead of sending the voice to Apple's servers.

#### Tasks view and per-task Stop
- What: a list of running and recent tasks with exact per-task Stop, recent activity, results, artifacts and the full
  record. Inspection never stops anything.
- Status: **Shipped.**

#### Attachments and results as objects
- What: attach files (sent as source paths, never as extra authority); paste with provenance kept; results with
  Open, Quick Look (Space) and Reveal in Finder. The "Is this your request?" paste dialog is removed.
- Status: **Shipped**; paste-dialog removal **integrated, not live-tested**.

#### Drafts never stored
- What: the unsent draft is kept only in memory, never written to the app's preferences file. A sent message is saved
  in the app's history only as the daemon's redacted copy (`ack.message`), so a secret typed into a chat message never
  reaches the app's archive; a message sent before FERB confirmed it is saved as a placeholder.
- Status: **Landing this week — integrated, not live-tested (L20, L7).**

#### Packaged FERB.app in a DMG
- See 3.15. **Landing this week — integrated, not live-tested (L27).**

### 3.2 The CLI (developer and power-user tool)

#### `ferb chat`
- What: terminal chat that never blocks; tasks keep running when chat closes. `--new`, `--resume ID`, `--verbose`
  (plans, steps, tools, checks), `--speak` (the Mac's voice reads final replies). Escape on an empty line stops the
  latest task; `/stop` stops a named task or all; Ctrl-C twice on an empty line closes chat.
- Status: **Shipped.** The daemon's ack in the CLI and the removal of the old CLI shortcut that auto-answered the only
  pending approval when typed text matched an option (a payment included): **Landing this week — integrated, not
  live-tested (L7, L19).**

#### Command reference
All **Shipped** (`ferb --help`):
- `ferb trace [TASK] [--steps|--full|--json]`, `ferb logs [--tools]`, `ferb timeline`, `ferb stats`, `ferb doctor`,
  `ferb baseline [--compare A B]`, `ferb verdict TASK pass|fail`, `ferb regress`, `ferb release-checklist`,
  `ferb test-mode`.
- `ferb stop` / `ferb resume-tasks`, `ferb pause` / `ferb resume` (sensing), `ferb ps` / `ferb kill ID`,
  `ferb attach ID [--take]`.
- `ferb reminders`, `ferb schedules`, `ferb memory …` (list, show, add, confirm, retract, edit, forget, dashboard,
  digest, consolidate, search, identity, rollback, eval-export, embeddings), `ferb restore [ID]`.
- `ferb conversations …`, `ferb notifications …`, `ferb browsers …` (pair, extension install/path/package),
  `ferb device show|scan`, `ferb runtimes [use ROLE RUNTIME]`, `ferb decide stats`, `ferb voice on|off`.
- `ferb setup [--update|--persona|--identity|--key P|--jev|--codex-login|--voice]`, `ferb uninstall [--purge]`.

### 3.3 Perception (always-on sensing)

#### Passive sources
- What: workspace and app-focus events, window and browser-tab metadata, idle, app installs, an Accessibility
  observer per app, configured folder events, typed text stored **per field** ("typed X into field F of app A",
  never per key), clipboard changes, macOS Notification Center records (via Full Disk Access), and installed-software
  facts. Grouped by app, then window/tab (`ferb/sense/perception.py`, `ferb/sense/watchers/`).
- How: pause and exclusions apply first, then code redaction, then actor tagging (owner / FERB / unknown), then the
  local append-only store. Secure text fields, Secure Input, terminal apps, concealed or transient clipboard types are
  never captured.
- Status: **Shipped** (F7, installed). Typed-text capture recorded nothing until the launch-week fix, which also
  stops capture in terminal apps and of shown passwords: **Landing this week — integrated, not live-tested (L17)**.
  Owner
  acceptance of the two-app Accessibility prototype, field capture, clipboard and notifications is pending.
- Limits: "now playing" (what music/video is playing system-wide) is not built. Background-task inventory is
  unavailable (macOS's only source opens an admin prompt). Electron/browser custom controls vary.

#### Jev batching and FERB's notices ("understand before telling")
- What: about every 10k tokens of redacted activity per app, a batch goes to **Jev**, an optional hosted
  decision model. If Jev says "worth attention", FERB runs one consideration per app at a time (later batches for
  that app merge into the next one), with the owner's relevant memory, recent chat, running tasks and notices already
  told. It then posts what happened and what it means for the owner, in its own words ("You upgraded Claude to Max
  20x. It renews on Nov 13."), or nothing. A notice can never create a task or a send.
- Status: notices: **Shipped** (F7/F9) as macOS banners; delivery in chat, one moment one notice, owner context,
  quotes with origins, "already told" that actually works, no backlog flood at install: **Landing this week —
  integrated, not live-tested (L9, L10).**
- Requires: a Jev key (`ferb setup --jev`). Without it, activity is still recorded but no notices are produced.

#### Notification triage
- What: a separate Jev question that would decide which incoming notifications deserve the owner's attention.
- Status: **Shadow only** (recorded and measured against owner labels; it never sends anything). Enabling it is an
  owner decision after measurement.

#### Notification rules (mutes)
- What: "don't tell me about this group" is saved as a rule quoted from the owner's words (app, chat, topic,
  expiry, exceptions); `ferb notifications rules|unmute|label`.
- Status: **Shipped**; case-insensitive app ids **integrated, not live-tested (L9)**.

#### FERB knows its own actions
- What: events in a window or tab where FERB is acting, or in FERB's own browser profile, are recorded as FERB's, so
  FERB never narrates its own clicks back to the owner. Clipboard writes by FERB are marked as FERB's.
- Status: **Landing this week — integrated, not live-tested (L9, P4).** Clipboard attribution: **Shipped.**

#### New-software watch
- What: when software is installed, FERB records signer, Gatekeeper assessment, provenance, entitlements, readable
  privacy grants and associated launch agents, as facts the agents can judge ("warn me about suspicious software").
- Status: **Shipped** (installed and observed running 2026-10-11). Owner acceptance pending.

#### Daily summaries and activity search
- What: a code-built daily summary (apps per hour, sites, window titles, FERB tasks) plus a model-written narrative,
  kept forever and searchable by content; today's matching activity goes into each planner turn.
- Status: **Shipped.** One indexed search per turn, off the event loop: **integrated, not live-tested (L28).**

#### Retention, purge and exclusions
- What: raw activity 30 days (a day is purged only after its summary exists); typed text and clipboard content 7
  days (including derived notice text: **integrated, not live-tested, L9**); detailed traces and Jev decisions 30
  days; captured
  images 7 days; task stories, the task list, chat, daily summaries and memory kept. Exclusion lists (apps, domains)
  and `ferb pause` apply before storage.
- Status: **Shipped.**

#### Setup persona
- What: on first open, FERB reads local app-use metadata (Spotlight use counts) and proposes a short profile with its
  evidence; the owner saves, edits or skips. Installed apps alone never establish an occupation.
- Status: **Shipped**, live (the owner reviewed it 2026-10-11). "Asked once, no banner": **integrated, not
  live-tested (L3).**

#### Desktop disturbance sentinel
- What: throughout every task, FERB samples the front app, key window, cursor, full-screen state, window order and
  Space changes, and records each change with the nearest FERB on-screen action and the FERB-started processes
  running. It states facts, never who caused a change.
- Status: **Shipped, live-verified (F4).**

### 3.4 Seeing (screenshots, front window, recordings)

#### Start-of-task picture and screenshots on request
- What: the model gets one picture of the working app's window (never the whole desktop) when a worker first works
  on it, then screenshots only when it asks (`mac.screenshot`, `browser.screenshot`).
- How: password fields (from Accessibility and the extension) are painted over, and on-device Vision OCR blacks out
  recognised secret classes before any pixel reaches a model, evidence file or recording.
- Status: **Shipped** (F5).
- Launch-week change: browser windows, task-tab screenshots and recording frames go to the model **redacted, with no
  exceptions**; nothing is refused any more, and an image is withheld only when masking itself fails; shown
  passwords are masked too. **Landing this week — integrated, not live-tested (L13).** Before it, browser windows and
  recording frames were withheld from the model.
- Fix wave 4 (2026-10-19): a window picture (`mac.screenshot`, the front window, a recording frame) whose secret
  fields FERB can't all locate (the window's browser profile isn't paired, Accessibility can't read the page) is no
  longer withheld: the fields it did locate are painted, the rest is masked by on-device text recognition only, and
  the picture says so with the reason. A window picture is withheld only when the masks can't be painted on it.
  Not live-tested. Rounds 2–5 (extension 0.15.6): a `browser.screenshot` (or recording frame) of a task tab whose
  page the extension can't script (a browser error page) is sent too, painted from Accessibility when the tab is the
  one its window shows, and masked by text recognition, with a note; a page the debugger can't attach to (the Chrome
  Web Store) gives a picture of the whole browser window when its tab is in front and its window on screen, and is
  withheld with the reason otherwise. A secret the extension found but can't cover also still withholds.
- Limits: OCR can miss unreadable or obscured text; this is class-based redaction, not removal of all private text.
  Where fields couldn't be located, a secret the OCR doesn't recognise as one (an unlabelled shown password) can
  show.

#### Front window with every message
- What: see step 3 of §2. Answering screen questions directly from the screenshot (a reply, no worker, no checker).
- Fix wave 4 (2026-10-19): a browser's URL and tab title are those of the window actually in front (matched by its
  place and title); when FERB can't tell which window that is, it sends no URL rather than another window's.
- Status: **Landing this week — integrated, not live-tested (L12).** Privacy: the hosted model sees the front window,
  redacted, with every message; the owner accepted this.

#### Target recordings
- What: record specific task tabs (`browser.record`) or a native window (`mac.record`) as silent sampled video
  (2 frames/s by default, 0.2–10 configurable), bound to that tab or window even when the owner uses another app.
  `recording.stop` writes an MP4 plus a receipt; `recording.inspect` decodes chosen moments into a contact sheet the
  model must look at before claiming something was recorded.
- Status: **Shipped** (offline-tested; the live two-tab check is pending). Needs `ffmpeg`/`ffprobe` installed.
- Limits: not high-frame-rate video; recordings keep no audio track (a transcript can be attached, 3.5).

### 3.5 Hearing (during tasks)

#### `audio.listen`
- What: a worker can listen to one app (or the browser showing a task's tab) for as long as it chooses, and gets a
  redacted, time-stamped transcript labelled as outside content. Listening ends with the task.
- How: ScreenCaptureKit captures that app's audio (FERB's own audio excluded); FERB.app transcribes it with Apple's
  **on-device** speech recognition (no cloud fallback: if on-device isn't available, the tool says so). It works in
  the languages your Mac recognizes on-device, which differ per Mac (on the owner's Mac en-US and en-IN do, hi-IN
  doesn't, measured 2026-10-14). The sound
  file is deleted as soon as it is transcribed; only the transcript is kept (owner decision 2026-10-14).
- Status: **Landing this week — integrated, not live-tested (L26).** Needs a one-time Speech
  Recognition permission for FERB (setup asks; doctor reports).
- Limits: capture is per app, so listening to a browser hears every tab of that browser that makes sound. The sound
  must actually play, so FERB may briefly unmute or play a tab the owner can hear (recorded like any on-screen
  action). System mute behaviour is unconfirmed.

#### Recordings with a transcript
- What: `with_audio` on a recording attaches the redacted transcript aligned to the video's timeline.
- Status: **Landing this week — integrated, not live-tested (L26).**

#### Media control
- What: `browser.media` plays, pauses, mutes, unmutes or seeks a page's video/audio; snapshots report each media
  element's state (playing, paused, time, duration, muted); "playing" means the media's time is moving, so FERB
  confirms a video is really playing before recording or listening. (In a real task FERB recorded a reel that never
  played; this fixes that.)
- Status: **Landing this week — integrated, not live-tested (L26)**; needs extension 0.15.6 / protocol 17.

#### Always-on hearing
- Status: **After launch / not decided.** The owner chose hearing during tasks only, not 24/7.

### 3.6 Browser control (Chromium extension)

#### Extension, pairing and profiles
- What: FERB's MV3 extension (`extensions/chromium`, version 0.15.6, protocol 17) is FERB's hands in Chrome, Brave,
  Edge, Chromium and other Chromium browsers. It talks only to the FERB app through native messaging, only in paired
  profiles. On tabs a FERB task opened or claimed it reads pages (text and accessibility tree), clicks, types,
  scrolls, uploads, downloads, handles dialogs and takes tab screenshots. In paired profiles it also reports every
  page load and tab switch (URL with secret-looking parameters removed, and title; never incognito) for FERB's local
  activity history, and, before FERB screenshots a browser window, finds where password and code fields are on the
  visible tab (positions only) so they can be painted over. Arbitrary script evaluation is not allowed (a fixed
  DevTools command allowlist; its one script-evaluation command is pinned to one bundled function, the page hook).
- Profiles: FERB's own browser profile (FERB's own accounts, e.g. its Gmail/Discord) by default; the owner's personal
  profiles when the owner asks ("my scaler Gmail", "my current Brave tab"). The model picks from candidate profiles;
  no code guesses the account.
- Background: task tabs open inactive in an existing window; creating a new window needs explicit foreground
  permission (it can switch Spaces). Tabs FERB opened close at the end unless kept open on purpose; borrowed tabs are
  released, never closed.
- Status: **Shipped.**

#### Downloads without dialogs
- What: a download in a tab FERB is driving never opens a Save dialog; the file goes to
  `~/.ferb/workspace/downloads/` and the task gets facts (path, size, SHA-256, source URL), labelled outside content.
  Owner-set limits: 512 MB per file; 50 files / 4 GB per task; 50 new tabs per task; 20 GB folder (oldest to Trash
  only past it); 10 GB free-disk floor.
- Status: **Shipped** (F4; limits live-verified).
- Limits: several edge cases can still reach the browser's own download flow (listed in `README.md`).

#### Uploads from FERB's own copy
- What: FERB copies the file into its workspace (APFS clone) and gives the browser the copy, so macOS never asks the
  owner for Desktop/Documents/Downloads access mid-task; the site sees the original name; the original is only read.
- Status: **Shipped** (the live re-check is pending).

#### Sealed password and one-time-code fills
- What: when a login needs a password or code, FERB asks the owner through hidden input; the value is filled
  straight into the exact field on the exact HTTPS origin (or exact native app field) and never reaches a model,
  trace, log or the database. Split one-digit code boxes are supported. The owner can choose to store one item in
  the Keychain; models then see only a handle. FERB can also read codes from FERB's own inbox via opaque candidates.
- Status: **Shipped** (F5). Shown passwords ("Show password" ticked) are now treated as secrets everywhere:
  **integrated, not live-tested (L17)**. The real Zomato split-code login failed before the split-box fix and has not
  been retested
  live since.

#### Extension auto-install at setup and Web Store
- What: `ferb setup` finds every installed Chromium browser and profile, registers FERB's native host, writes the
  Chrome Web Store install request where it is documented to work (Chrome/Chromium; Edge and Brave "unconfirmed";
  Vivaldi, Arc, Opera manual), and reports per profile whether the extension is installed, enabled and paired. It
  never edits browser preferences or restarts a browser; the owner enables the extension when the browser offers it.
  With the store request turned off (`[browser] store_extension_id = ""`), setup keeps the Load-unpacked steps.
- Status: **Landing this week — integrated, not live-tested (L25).** The listing went live on 2026-10-20 with 0.15.2
  (its id is built in).
- Known limit: the repository now requires extension 0.15.6 / protocol 17, and the daemon refuses an older protocol.
  The store copy must be updated to 0.15.6 before the store install path works with the launch build.

### 3.7 Mac app control

#### Accessibility-first native control
- What: `mac.snapshot` (read the accessibility tree), `mac.act` (press, set value, edit, type into an exact element),
  `mac.click` / `mac.pointer`, `mac.menu`, `mac.open`, `mac.apps`, `mac.screenshot`, `mac.clipboard`, `mac.secret`,
  `mac.applescript` (sandboxed, can drive apps the owner authorised for Automation). After each action the tool reads
  the result back through Accessibility; a delivered click is never taken as proof.
- Background rules: Accessibility actions on the exact window first, then app-directed events, non-activating
  scripting, URL schemes; global keyboard/mouse input needs the owner's direction. FERB never types into terminal
  apps.
- Status: **Shipped.** Live evidence: TextEdit background edit verified (0.26); WhatsApp chat reading used live (F1
  pass). Most other apps are untested per app; background behaviour depends on the app and macOS.
- Fix shipped 2026-10-11: an Automation permission check (Spotify) can no longer freeze the daemon.

### 3.8 Shell (sandbox, proxy, escape hatch)

#### Shell and coding sessions
- What: `shell.run`, `shell.start` (background/interactive PTY), `shell.send`, `shell.read`, `shell.wait`,
  `shell.kill`, `shell.list`, `shell.rules`. FERB can run and supervise coding agents (e.g. the Codex CLI) inside its
  containment and approve their prompts (recorded as FERB's). `ferb attach ID` lets the owner watch and take over.
- Status: **Shipped.**

#### Sandbox
- What: every shell command and everything it starts runs under a macOS sandbox profile: no reading credential stores
  (Messages, Mail, browser cookie/login stores, Keychains, `~/.ssh`, `~/.aws`, …), no reading FERB's own files, no
  writing protected places (`~/.ssh`, LaunchAgents, shell startup files, `/System`, `/Library`, `/Applications`), no
  Keychain, no opening apps or URLs, no background jobs, no Apple Events, no mounting. If the sandbox can't run, the
  shell tools are switched off (never uncontained).
- Status: **Shipped** (F5). Live security runs pending.

#### Egress proxy: any site, every site the shell reaches recorded
- What: the shell's only way to the internet is FERB's proxy, which records host, port and bytes per command (never
  page, path or query). Programs that ignore proxy settings get no network.
- Launch-week change: shell and browser reach any site **without asking**; code records every site the shell reaches
  and every page a task's tabs load (L2, I6). Before, 65 of
  100 approvals in the owner's first 25 tasks were shell-network questions (one research task asked 46 times in 7
  minutes).
- Status: **Landing this week — integrated, not live-tested (L2)**, with "Sites contacted" at the end of the full
  answer.
- Local services: an existing local server is reachable only when the owner pins its port to an executable
  (**Shipped**); direct connections to pinned ports, recorded from the kernel's reports: **integrated, not live-tested
  (L24).**

#### Escape hatch
- What: for code-enforced boundaries only (writing a protected folder, reading a never-read store, an admin `sudo`
  command with the password through hidden input, a certificate-service lift for one Go TLS command). FERB shows the
  exact command, folder and rule, and asks once for that exact launch.
- Never possible, even with the escape hatch: changing FERB's own audit records, protection rules, sandbox profile,
  config or secrets helper; typing into terminal apps; bypassing the proxy.
- Status: **Shipped** (F5). Still asks after launch week.

#### Snapshots before shell work
- What: before shell commands FERB takes a local APFS snapshot (at most one per 15 minutes; deleted after 7 days).
- Status: **Shipped, live-verified (F4).** Limit: FERB cannot open a snapshot itself; the owner can via Time
  Machine/Recovery. Shell file changes get no per-file copy.

### 3.9 Files and recovery

#### File tools
- What: `fs.list`, `fs.read`, `fs.write`, `fs.edit`, `fs.trash`, `fs.disk_usage` (sizes, types and dates only,
  never contents). No deliverable size cap.
- Status: **Shipped.**

#### Recovery copies, Trash and `ferb restore`
- What: before FERB overwrites or edits a file it keeps an APFS-clone copy for 7 days; deletions go to the Trash
  with their origin remembered. `ferb restore` lists what can be put back; `ferb restore ID` restores without ever
  overwriting a change the owner made since ("<name> (before FERB)").
- Status: **Shipped, live-verified (F4).** Trash moves only between folders FERB holds open, nothing moved out of the
  Trash after a mismatch, and no Trash writable by the shell: **integrated, not live-tested (L27)**. Finder's "Put
  Back" isn't offered for FERB's items; `ferb restore` is the way back.

### 3.10 Scheduling, timers, reminders, standing permissions and long plans

This section matters most for an automated campaign. It says exactly what runs unattended.

#### Reminders and timers
- What: "remind me at 8:10" or "in 20 minutes" (`time.remind_at`, `time.set_timer`). A reminder only reminds; it
  never runs a task. The owner's words are quote-checked; times are computed by code from the owner's wording, with
  no hidden 09:00 default.
- Status: **Shipped.** Accepting a reminder asked for in an earlier message of the same chat, and keeping the reply
  when a reminder is refused: **integrated, not live-tested (L11).**

#### Schedules (unattended runs)
- What: "do X tomorrow at 9" or recurring (daily, weekdays, weekly, every N minutes). Each run is a fresh,
  **unattended** task with the original request and targets. `ferb schedules` lists, cancels, pauses, resumes.
- What happens when nobody is at chat: any step that would need the owner (a code confirmation, a question, a
  foreground permission) is **not done**; the run reports what is missing. Standing permissions from an earlier chat
  task do not carry into a scheduled run. A standing scope stated when the schedule is made is recorded with it, but
  only for destinations the owner typed (name the real domain and handle: "x.com", "@handle"; "X" isn't "x.com"); each
  run gets exactly that scope and nothing more, and cancelling or pausing the schedule ends it.
- Status: **Shipped.** Scheduled-run failures named in chat instead of a generic apology; account errors never drop a
  request silently; scopes recorded with a schedule: **Landing this week — integrated, not live-tested (L21, L3).**
- Limits: the Mac must be awake and logged in, with the daemon running; a run that comes late is judged by the
  agents for usefulness. An unattended browser task needs the browser and its paired profile already open with a
  window (creating a window needs the owner's permission).

#### Watches ("tell me when …")
- What: "tell me when ~/Downloads/report.pdf exists", "alert me whenever the battery is at or below 10%". The planner
  makes a schedule that checks every N minutes and marks it a watch that ends once it happens or a standing one
  (stored with the schedule, so it survives restarts and `ferb setup --update`; a loop inside a task did not). Work
  asked for in the same message ("turn on Low Power Mode") is a task that runs now. Each check is an unattended run
  whose worker reports whether the condition holds now, and its final check judges that report. A met condition
  reaches the owner as a "FERB alert" notification, as a reminder does (even with the app open or the owner away),
  once each time the condition becomes true (judged against the last check that reported it); a check with nothing
  new to tell is recorded but reaches no client. A confirmed check that finds a one-shot watch's condition met ends it
  (its recorded send scope withdrawn), and that answer says FERB stopped checking; an unconfirmed or failed check
  never does. "Tell me when the agent you started is done" is the task's own monitoring, not a watch. `ferb
  schedules` shows each watch's kind.
- Status: **Landing this week — integrated, not live-tested (fix wave 6c, live check 76).**
- Limits: the interval is the schedule's (the planner uses every 15 minutes or more); no battery, power or file event
  exists, so a crossing is seen at the next check. A loud alert is the run's own doing with its tools when the owner
  asked for one; code adds no sound. A check resumed as a saved plan after a restart, or interrupted by one, is told
  as any task's.

#### Durable plans with waits
- What: multi-step plans with step types do / map / reduce / wait / ask / converse; waits on a time or an event
  (e.g. a reply); completed steps stay complete across daemon restarts; unknown outcomes are never replayed.
- Status: **Shipped** (F6; live acceptance pending). Waits now fire on wall-clock time after the Mac sleeps:
  **Landing this week — integrated, not live-tested (L22).**

#### Standing permissions (D30)
- What: for a long task the planner can propose exact send scopes (tool, account identity, site, recipients, and
  either an exact message body or explicit "any message body"). Inside the approved scope, sends that code would
  otherwise ask about (new recipient, a new destination after private and outside content, bulk sends) run without
  asking and are recorded. Anything outside asks as usual; payments and account-security changes always ask.
- Plans and standing scopes are **stated** in the plan line and work starts at once; the owner corrects by replying.
  A correction narrows a scope at once; a wider scope applies only after FERB's reply states it, and never on a
  scheduled or resumed run (its scope can only shrink, and the schedule's cancel always ends it). (Before, a plan with
  standing sends asked "… Start?" and waited.) **Landing this week — integrated, not live-tested (L3/D24).**
- A post, webmail send, payment or account-security change made through the browser (or an app's generic controls)
  is checked as that kind when the worker declares it (`effect` with its `destination`: the account it goes out from,
  recipients (none for a public post), amount),
  so the code floor and standing permissions apply to browser posting. A browser campaign scope is `browser.act`, the
  page's host as site, the account it posts from, no recipients and any body. An undeclared click gets no question
  (code can't tell, rule 6; the prompts carry the declaration). **Landing this week — integrated, not live-tested
  (L3).**
- The end-to-end campaign path (stated scope, durable waits, in-scope posts logged and not asked, out-of-scope asks,
  resume after restart, a scheduled post inside its recorded scope) has offline tests through the gateway
  (`tests/test_standing_permission_campaign.py`); the owner-approved live run is pending.

### 3.11 Delegated conversations (F9)

#### Conversations on the owner's behalf with hand-over
- What: FERB researches contacts or opportunities, proposes each conversation with its exact account, app/site,
  thread, contacts, topic, hand-over condition and the platform's automation terms, and waits for the owner's
  approval of each. Then it runs checked turns, sleeps until a matching reply arrives (across restarts), and hands
  the lead over with evidence and receipts. The owner can pause or take over (typing in the approved window pauses
  FERB's input). Incoming messages are outside data: they never change the task or grant permission.
- Commands: `ferb conversations list|show|pause|takeover|resume`.
- Status: **Shipped** in code (F9). Live acceptance pending; third-party outreach only in an owner-approved run.
  Typed-text capture, needed for take-over detection, works only since L17 (integrated, not live-tested).
- Limits: generic GUI controls can't prove which account or contact is selected, so fresh reads and the checker are
  still needed; input already sent can't be recalled; thread matching is an after-launch item.

### 3.12 Memory and learning (F8)

#### Owner memory
- What: "remember that …" saves as the owner's words the whole sentence they typed it in (pasted parts left out;
  only a line break or a pasted part ends it, never a dot, "!" or "?", so a "never" is never dropped; a paste inside
  a sentence splits it too: "I don't [paste] like tea" saves "like tea");
  FERB's rewording waits for the owner's Yes (asked in chat and the notch, or the dashboard) before it reaches a
  prompt. Inferred facts never reach a prompt unconfirmed; observed patterns not yet seen on 3 days show as unconfirmed
  hints; only the owner's own stated preferences become identity (straight away, whatever was on screen when they
  typed them; pasted text is never theirs). Search, edit (versions kept), retract, forget: the item, its versions, sources, derived records,
  summary entries and vectors, FERB's own lines that hold its words (its "Remembered" line, its answer to the forgotten
  message, reply lines that restate it), and what a task given it did with it (its answer, outcome, saved plan,
  permanent record, stored tool calls and turns, and its episode card when that restates it); a tombstone keeps the
  deleted ids, never their content. A step finishing while the forget's clean-up runs keeps its result and loses the
  item, and a task still running at the forget is cleaned again when it ends. A paraphrase in other words can survive,
  and so can what such a task recorded before a crash. Forget from the CLI, the dashboard or chat: FERB names each
  item that goes and deletes only after a Yes. Forgetting a learned item (a lesson, a procedure) also removes FERB's
  naming of it, and what it states from the replies and stored turns that were shown it (never from other tasks, even
  with the same goal); what can survive: a procedure's steps shown to a worker (in that task's stored
  turns), records deleted only because they were built on it (deleted, but their words not scrubbed), a lesson's scope
  labels (app, site, intent), and an entity summary's multi-line text unless it was recorded exactly.
- Status: **Shipped**; the chat forget and the scrub of FERB's own lines are **landing this week — integrated, not
  live-tested (fix wave 3)**. The Mac app's own chat archive still keeps its copy of a forgotten line.

#### Similar past tasks and recall
- What: before planning, FERB retrieves relevant facts, past episodes, procedures and lessons (keyword search that
  keeps Devanagari words whole; optional local embeddings only if an owner-labelled evaluation shows they help).
- Status: **Shipped**; no embedding model adopted yet.

#### Episodes
- What: one permanent record per task, composed by code from the trace. Its learning card cites the task's own
  sources only (its request, its tool results and evidence, the procedure it used), so retention and forgets of other
  conversation entries never stop learning (fix wave 3).
- Status: **Shipped.** 35 task records had failed to save because redaction corrupted internal ids: fixed
  **Landing this week — integrated, not live-tested (L16).**

#### Procedures, lessons and the fast path
- What: an idle-time consolidator writes procedures (tool-call templates with expect lines) citing verified
  episodes; a step some runs skipped is optional and must reproduce the runs that cite it, every other step every
  run (L29). A procedure becomes "learned" after three verified successes across two different slot values on the
  current app versions (a run's receipt without slot values doesn't count as a second value). A learned procedure can
  save exploration but never skips gateway checks, step checks or the final check. Procedures with steps that came
  from outside content are quarantined until the owner confirms. An app update sends a learned procedure back to
  candidate until it succeeds on the new version; two failures in a row mark it suspect. A suggestion the owner turned
  down ("Not right") is never proposed again. The consolidator reads only cards whose inputs changed, has the
  learning model label each card with its intent (what kind of task it was, reusing a kind already seen when the goal
  is the same; L29), and consolidates by intent: an intent is first learned once it has three verified runs, and from
  then on every change to its runs (a new run, verified or failed, or a verdict) brings another call to revise its
  procedure and lessons; one call per intent carries its procedure, its lessons and its runs, whatever tools they used
  (the procedure's example runs first, then the newest, as many as fit the model's window) (L29). A revised lesson
  keeps the owner's helpful/harmful ratings; a procedure the owner forgot or turned down is never learned again for
  that intent. A model call that failed is tried on the next pass; an answer outside the schema, a call whose every
  proposed procedure was refused (its job says why), or input too large for the window, waits for a change or
  `ferb memory consolidate`. A planner's procedure id that isn't a current procedure is ignored with a
  note.
- Status: **Shipped** in code, but all consolidator runs failed and the hourly loop crashed until the fix
  **Landing this week — integrated, not live-tested (L16)**. No learned procedure has been observed live yet; do not
  claim measured
  speed-ups from learning.

#### Memory dashboard (P17)
- What: `ferb memory dashboard` prints a private localhost link (token in the URL fragment) to review identity,
  facts, past tasks, procedures, lessons and rollups with sources; confirm, edit, forget or roll back.
- Status: **Shipped**; live check pending.

#### Running conversation summary
- What: when chat history no longer fits the model's window, FERB keeps a summary of the older part; the full text
  stays fetchable.
- Status: **Landing this week — integrated, not live-tested (L28).** It refreshes only when entries it doesn't cover
  fall out of the window.

#### Skills directory
- What: owner-written skills in `~/.ferb/skills`; a small catalogue is visible to workers, bodies are read
  explicitly, with no symlink escapes.
- Status: **Shipped.**

### 3.13 Security and privacy model

#### Direction: trust, record, recover (L1)
Prevention is a small code-enforced core this week; a fuller design follows after launch (§7). Everything FERB does
is recorded and FERB's own file changes are recoverable.

#### What still asks the owner (code-enforced unless noted)
Each question names the exact step, is asked once per task, and the answer carries across that task's retries; a No
is final for that step in the task.
- **Payments** and **account-security changes** (passwords, sign-in, recovery settings): always ask, and each Yes
  covers one attempt. Code fact: no tool is a payment by its own kind (a browser "Pay" click is a click); the gateway
  checks a click or typing step as a payment or account-security change when the worker declares it (`effect`), on
  any acting tool. Workers no longer ask for confirmation themselves. Treat payment protection as "code floor when the
  worker declares the step" (an undeclared click isn't caught, rule 6) until the live checks confirm it.
- **Permanent deletion** of something the owner didn't name (a folder delete shows count, size and a sample). FERB's
  own file tools only move to the Trash, which is recoverable and does not ask.
- **Sending as the owner to someone new** (a recipient or contact the owner didn't name), unless a standing
  permission covers it. Code fires this for native-app sends declared as send/call and for browser posts and webmail
  sends declared as `effect: send` (see 3.10); a post to a site and account the owner named is already authorized.
- **Private data to a place the owner didn't name after the task read outside content** (S4): a send to a new place
  after reading both the owner's data and outside content; a URL carrying an actual private value to a host nobody
  named; a recipient, number or amount that only outside content supplied. False alarms from FERB's own context words
  removed: **integrated, not live-tested (L4)**.
- **Bulk sends** to several new recipients: one question with count and sample (**integrated, not live-tested,
  L3**).
- **Escape hatch** commands (D31).
- **Retrying an action whose outcome is unknown** (P19), such as a send that was cut off: "an earlier attempt … may
  already have happened". A Yes allows exactly one more attempt.
- **Installing a tool the owner named that isn't installed**: asked once as a courtesy, with the install command
  prepared (owner correction, 2026-10-14). **Integrated, not live-tested (L3).**
- Ordinary what/who/which questions when evidence leaves real ambiguity (agents decide; never "how should I do it").

#### What never asks
- Reaching any website from the shell or browser (recorded instead) — **integrated, not live-tested (L2)**.
- Big-job plans: stated in the plan line, work starts at once — **integrated, not live-tested (L3)**.
- Reversible work: reading, navigating public pages, writing/editing files (copies kept), moving to the Trash.
- Notices, memory reads, recordings and listening during a task.

#### Secrets never reach models
- Sealed path for passwords, codes and admin passwords (hidden input, filled directly; models see handles only).
- Code redaction at capture for secret classes: private keys, API keys and tokens (OpenAI, Anthropic, GitHub, Slack,
  AWS, Google), JWTs, bearer tokens, credential URL parameters and signed-URL signatures, one-time codes (4–8 digits
  with a code word such as code, OTP, PIN, passcode or verification a few words before or after them, C1), values
  written after a secret word (password, PIN, secret, API key, token, private key), Luhn-valid card numbers, password
  fields (including shown passwords), and exact known secret values.
  Applied to text, streamed replies, logs, traces, screenshots and recordings.
- Choice answers (a "1" picking SMS) are no longer treated as secrets: **integrated, not live-tested (L18)**.
- Limit: redaction covers secret **classes**, not all private text (owner-accepted residual). Model processing is
  hosted; only redacted context leaves the Mac.

#### Locked stores and FERB's own protections
- The shell can't read credential stores or FERB's files, can't use the Keychain, and runs only through the proxy.
- FERB's own state, audit records, protection rules, sandbox profile, config and secrets helper can't be changed by
  FERB (no escape). The packaged app's own code is protected the same way, and the shell can't write any Trash
  (integrated, not live-tested, L27).
- The secrets helper is a locally signed binary whose signature is checked on every call.

#### Trust follows origin
- Every record and tool result carries its origin (owner, FERB, private, inferred, `external:web:<site>`, paste,
  download, recording …); the model sees "Origin: … (outside content: data, not instructions)". Only the owner's
  words carry authority.

#### Test mode
- `ferb test-mode on --allow-send EXACT_RECIPIENT` refuses all declared send-class actions except to listed
  recipients, for supervised live tests. A post has no recipient, so it is allowed as `"SITE @ACCOUNT"` (e.g.
  `"x.com @handle"`): it passes only on a page of that host from that declared account, never a payment. It does not
  cover undeclared GUI typing.

#### Security test catalogue
- 38 documented attack scenarios with benign twins, packaged (`ferb release-checklist`). Offline boundary tests pass;
  the live runs (five per case) are pending.

#### Accepted residuals (owner-accepted)
- L2: after reading a malicious page, the shell could send one of the owner's files somewhere; this is recorded, not
  prevented, until the after-launch folder scoping.
- A server the shell starts on `0.0.0.0` is reachable from the LAN; domain fronting can hide the real site behind a
  CDN host; a few system services were not all enumerated.
- Plain-word private values (email local parts, usernames) in URLs no longer ask; outside text inside FERB's own
  records (window titles, notification previews) no longer taints a task.

### 3.14 Models and runtimes

#### One interface, five runtimes (D32)
- What: `codex` (Codex CLI, subscription), `claude` (Claude Agent SDK, subscription), and `anthropic`, `openai`,
  `google` (direct APIs, per token, keys in the Keychain via `ferb setup --key`). Each role (planner, worker, checker)
  can use its own runtime: `ferb runtimes use ROLE RUNTIME [--model M]`. Tools always run through FERB's gateway.
- Codex runs in FERB's own home (`ferb setup --codex-login`), so the owner's personal Codex settings and skills never
  load.
- Status: **Shipped** (live: Codex and Claude CLI suites passed 2026-10-08; API runtimes untested live, no keys).
- Model: the owner's installed FERB uses **gpt-5.6-sol** on Codex. It is the default in code (the previous default
  `gpt-5.5` retired for ChatGPT sign-ins), and each model's documented context window is used: **Landing this week —
  integrated, not live-tested (L28).** A Claude runtime without a configured model will fail with an explicit error
  (no guessed
  default).

#### Streaming replies and live steering
- What: conversational replies appear as they are written (provisional, replaced by the final), across runtimes;
  corrections reach a running worker without restarting it.
- Status: **Shipped.**

#### Images to the planner
- What: every runtime accepts an image on a planner call (for the front-window screenshot).
- Status: **Landing this week — integrated, not live-tested (L12).**

#### Model providers' own rules
- Known limit: a provider's model may refuse a request on its own (for example, it declined to extract a session
  cookie in one owner task; FERB's own locks would also have blocked it). FERB does not override provider refusals.

### 3.15 Packaging and install

#### Source install (today)
- What: `uv sync --group dev`, `uv run ferb setup`. Installs a dedicated interpreter, environment, pinned Codex,
  LaunchAgent under `~/.ferb`, collects macOS permissions (Accessibility, Screen Recording, Input Monitoring, Full
  Disk Access after containment is verified), builds the signed secrets helper, sets up browsers. `ferb setup
  --update` reinstalls and restarts; `ferb doctor` checks everything from inside the daemon.
- Requires: macOS on Apple silicon, Python 3.12 via uv, Node/npm, a signed-in Codex or Claude account.
- Status: **Shipped**, live (the owner's install).

#### FERB.app in a DMG
- What: one FERB.app (the notch UI as main executable, the daemon's Python and FERB inside, Codex and Claude CLIs,
  the secrets helper, the voice/transcription helper), in a DMG with an Applications link. First launch runs the same
  steps as `ferb setup`, including the offer to sign in to ChatGPT for FERB's own Codex (FERB can't answer without
  it; **Sign in to Codex…** in the menu bar runs it again), links `ferb` at `~/.local/bin`, and keeps the same
  `~/.ferb` home. The background
  daemon runs as an agent app named FERB, so macOS privacy settings show "FERB".
- Signing: **ad-hoc signed now**; the build has one signing step that becomes Developer ID signing plus
  notarization later, with no repackaging.
- Status: **Landing this week — integrated, not live-tested (L27).** A real build succeeded (app about 666 MB, DMG about
  187 MB). Not yet installed or launched live.
- Known limits: first open needs System Settings → Privacy & Security → **Open Anyway**; macOS may ask for
  permissions again after each update until Developer ID signing exists; untested whether launchd runs the bundled
  Python from a quarantined download (main risk; fallback options exist). Developer ID and notarization: **After
  launch.**

#### Uninstall
- `ferb uninstall` removes the LaunchAgent; `ferb uninstall --purge` also deletes `~/.ferb`. **Shipped.**

### 3.16 Observability

All **Shipped** unless noted.
- `ferb trace [TASK]`: the story in about 10–20 lines (steps, answer, problems, token/cache/approx-$ line, desktop
  changes, verdict); `--steps`, `--full`, `--json` (structured outcome export). Live-verified (F3).
- `ferb logs --tools`: one live line per tool call. Live-verified (F3).
- `ferb timeline`, `ferb stats` (outcomes, tool errors, model runs, shell latency, confirmations, escapes),
  `ferb doctor` (installation, permissions, containment, proxy, hearing, context windows).
- `ferb verdict TASK pass|fail`: the owner's acceptance; `ferb baseline`: metrics snapshots and comparisons (message
  → ack / plan / answer times added: **integrated, not live-tested, L8**); `ferb regress`; `ferb release-checklist`.
- Daily summaries include a security digest (new destinations, confirmations, blocked flows).

### 3.17 Performance (measured offline on replays of the owner's real data unless stated)

- **Before the launch-week work** (installed code, 62 real turns): median 17.7 s from message to first visible line
  (p90 31.6 s); 71% was the planner model call (median about 11.8 s of provider time), 26% FERB's own pre-planner
  work.
- **Acknowledgement:** sent on receipt, before routing (integrated, L7). Not yet timed live.
- **Message → plan line, FERB's side:** 11.4–17.0 s → **0.9–2.0 s** (replay of six real turns, provider time
  excluded; the model's own time comes on top). **Integrated, not live-tested (L28).**
- **Plan line → worker start:** 0.5–3.9 s → about 0.1 s (replay; integrated).
- **Reusable (cacheable) prompt prefix:** about 8k → about 54k tokens per turn (replay); real cache hits need a live
  check.
- **Live observation (F4):** shell tasks of 60 s and 90 s took 1 min 57 s and 2 min 29 s end to end (planning ~10 s,
  worker model turns 16–26 s, the check 25–32 s).
- **Idle CPU:** one watcher measured 96% → 0.24% CPU after the 2026-10-11 fix (that watcher only).
- Task end → outcome: a regression to about 2 s in an early L28 round was removed by capturing the episode card after
  the answer, in a separate process (a follow-up message's first line came in about 0.5–0.7 s during a capture, on
  the replay).

---

## 4. What FERB can do for its own launch campaign

Precise and conservative. "Today" means the code on `astra_resume`, launch-week work included (integrated, not
live-tested).

### Capabilities that apply
- **Post through the owner's logged-in browser profiles.** FERB can open X, LinkedIn, Reddit, Discord, Product Hunt
  or any site in a paired owner profile, type a post, attach images or video from FERB's own upload copy, click Post,
  then read the page back to confirm the post exists. **Shipped** browser primitives; no site-specific posting recipe
  or platform API exists.
- **Draft in the owner's voice.** The planner sees relevant memory (owner-stated preferences, identity rules) and the
  conversation; the owner can correct a draft by replying ("too LinkedIn-ish"), and corrections re-plan in the same
  task (**integrated, not live-tested, L11**). Store voice rules explicitly with `ferb memory identity '…'` or "remember
  that …" so every draft gets them.
- **Schedule posts.** Either (a) a schedule per post or a recurring schedule (each run unattended), or (b) one long
  plan with wait steps until each posting time (one task tree, durable across restarts and sleep; waits fire on
  wall-clock time: **integrated, not live-tested, L22**).
- **Watch for replies and mentions.** Ways that exist: a plan step that waits for a time and then reads the
  notifications/mentions page; a delegated conversation (F9) scoped to an exact thread that wakes on its incoming
  messages; perception notices in chat when Jev flags activity (needs a Jev key; integrated, not live-tested).
- **Reply to comments in scope.** Delegated conversations with an approved account/site/thread/topic scope and
  hand-over; live acceptance pending.
- **Research and monitor.** Read pages, search, collect metrics from analytics pages, summarise threads; the sites a
  task opened, the pages its tabs loaded (clicked results, and the landing page of a redirect) and every site its
  shell reached are
  listed under "Sites contacted" in the full answer (**integrated, not live-tested, L2, I6**).
- **Watch and hear media.** Play a video in a tab, confirm it is playing, listen and get a transcript (L26,
  integrated, not live-tested); record a tab as sampled video for evidence, with a transcript when asked.
- **Audit everything.** `ferb trace`, `ferb timeline` and the daily security digest show what was posted where; the
  owner records `ferb verdict` per task.

### What will still ask, and what won't (today's code)
- **A post or comment through the browser is checked as a send when the worker declares it** (`effect: send`, with the
  account it posts from in `destination`; Task 18). Then:
  - a post to a site and account the owner named in the request is already authorized and doesn't ask;
  - a post inside a standing scope (`browser.act`, the site's host, the account, no recipients, any body) runs without
    a question and is recorded, and the daily security digest lists it;
  - any other post asks once per task for that site and account, showing the text last typed on that page;
  - workers no longer ask the owner to confirm on their own; an undeclared click gets no question (code can't tell).
- **A scheduled campaign**: say the exact domain and handle when creating the schedule ("post … on x.com from
  @handle"). FERB records the scope only for destinations the owner typed; its confirmation lists what was recorded
  and what each run will still ask about. Cancelling or pausing the schedule ends the scope.
- **S4 can still ask** if a post's URL or recipient carries the owner's private data, or a recipient came only from
  outside content.
- **Native desktop messaging apps** (a send button declared as send): code asks for a new recipient unless a
  standing permission covers that exact app/account/recipient (and body, unless "any body" was approved).
- **Payments, account-security changes, logins needing a code** (the owner must type the code), escape-hatch
  commands and unknown-outcome retries need the owner.
- **Unattended scheduled runs** cannot answer any of these: such a step is not done and the run reports it.

### Current limits for campaign automation
- The Mac must be awake, logged in and running FERB; the browser with the paired owner profile should be open with a
  window (FERB can't create a window unattended without the owner's foreground permission).
- Platform automation rules: some platforms restrict automated posting and may suspend accounts; F9 shows platform
  terms for approval. This is the owner's risk decision.
- No image or video generation tool; FERB uses files the owner provides or that tasks download.
- CAPTCHAs, new-device checks and 2FA need the owner.
- The end-to-end standing-permission campaign path has offline tests through the gateway
  (`tests/test_standing_permission_campaign.py`, including a scheduled post inside its recorded scope) but no live run
  yet; plan the first posts as supervised runs, using `ferb test-mode on --allow-send "SITE @ACCOUNT"` with an
  owner-controlled account for dry runs.
- Model providers may refuse some requests.
- Recordings are 2 frames/s sampled video with no audio track (a transcript can be attached), not marketing-grade
  footage.

---

## 5. Demo-worthy scenarios

Real uses from the owner's traces (no private contents), with status.

| Scenario | What happens | Status |
|---|---|---|
| "Note down the assignment messages in my WhatsApp chat and read the linked doc" | Reads the chat in the background and the doc via its public text export; checker inspects the evidence | **Shipped, live-verified** (F1 owner pass) |
| "What is my disk storage going towards?" / free space | Metadata-only disk scan, answer with sizes | **Shipped, live-verified** (F0/F1 passes) |
| Restore a file FERB edited, even after the owner edited it too | `ferb restore` puts the copy back or beside as "(before FERB)" | **Shipped, live-verified** (F4) |
| Undo a Trash move; snapshot before shell work | Restores from Trash without overwriting; snapshot listed | **Shipped, live-verified** (F4) |
| "Which app came to the front and was it FERB?" | Trace states facts: "<App> came to the front — no FERB on-screen action in progress (the last ended 4.9 s earlier)" | **Shipped, live-verified** (F4) |
| Find the right assignment in a mail account and set reminders a day before the deadline | Correct answer and reminders (the owner failed the run for reading browser data via shell; F5's sandbox now blocks that) | **Shipped**; the original run failed for method, the fix is in F5 |
| Research a startup/topic online without a flood of questions | Shell and browser reach any site, recorded; zero network questions (was 46 in 7 minutes) | **Landing — integrated (L2)**; live check pending |
| "What does this line on my screen mean?" | Answered directly from the front-window screenshot, no worker (took 1 min 16 s through planner, worker and checker before) | **Landing — integrated (L12)**; not live-tested |
| "Find the startup he's talking about in this reel" | Plays the video, confirms it plays, listens, reads the on-device transcript | **Landing — integrated (L26)**; not live-tested |
| Subscription upgrade noticed | One understood chat notice ("You upgraded … It renews on …") instead of four alarm-style banners | **Landing — integrated (L10)**; needs Jev; not live-tested |
| Late answer to an earlier question | Appears at the bottom as a reply quoting the question | **Landing — integrated (L6)**; not live-tested |
| Correct a draft ("too LinkedIn-ish") | The new goal is used and re-planned | **Landing — integrated (L11)**; not live-tested |
| Reminder asked for earlier in the chat (a boarding-time timer) | Set, and the reply is kept even if one side output is refused | **Landing — integrated (L11)**; not live-tested |
| "Use tool X to clean my Mac" when X isn't installed | Asks once "X isn't installed — should I install it?", installs on Yes, continues | **Landing — integrated (L3)**; not live-tested |
| Kaggle: hand the work to a named coding agent and submit | FERB starts and helps the named agent, does none of its work, asks before choosing which submission is final | **Shipped** (F4 live fixes); live re-check pending |
| Upload a file from the Desktop to a web form | No macOS folder prompt; the site sees the original name | **Shipped**; live re-check pending |
| Food-delivery login with a one-time code into split boxes | Hidden input, sealed fill into the boxes | **Shipped**; failed live before the split-box fix, retest pending |
| Rank local coding-agent sessions and build a report page | Inventory, map/reduce fan-out, local site | **Shipped** (ran 2026-10-11); an audit found coverage was overstated, prompts tightened |
| Concert-ticket comparison | Multi-site research table | Ran; audit found unverified availability claims; prompts now require primary sources. Not a clean demo yet |

---

## 6. Known limits and honest caveats (consolidated)

Release and distribution
- The DMG is ad-hoc signed: first open needs "Open Anyway"; permissions may need re-granting after updates; the
  quarantined-launchd question is untested. Developer ID signing and notarization come after launch.
- No auto-updates; no in-app onboarding or settings beyond the setup console and sound settings.
- The Chrome Web Store listing is live with 0.15.2; the launch build needs 0.15.6 / protocol 17, so the store copy
  must be updated. Edge and Brave install requests are unconfirmed; Vivaldi, Arc and Opera need Load
  unpacked.
- Apple silicon and Python 3.12 only. The app is about 666 MB (bundled Codex and Claude CLIs).
- The privacy page draft of 2026-10-15 (pending the owner's approval, not deployed) names the optional Jev service at
  api.typesafe.ai, the extension's tab reports and secret-field lookup, local clipboard retention and on-device speech.

Acceptance
- F5–F10 owner live acceptance is pending; launch-week items are mostly not live-tested yet. Passing offline tests
  never proves a live workflow.
- The 38-scenario live security suite has not been run.

Security and privacy residuals
- Trust-record-recover: after a malicious page, the shell could send a file somewhere (recorded, not prevented).
- Payment, account-security, permanent-delete and send asks for browser and app input are enforced in code when the
  worker declares the step's effect; an undeclared click isn't caught (see 3.13).
- Redaction covers secret classes, not all private text; OCR can miss text.
- The hosted model sees a redacted screenshot of the front window with every message (L12, owner-accepted).
- LAN-reachable `0.0.0.0` servers, domain fronting, unenumerated system services, listener timing (see 3.13).

Capability limits
- Background behaviour depends on each app and macOS; most native apps are untested per app.
- Hearing is per app (a browser is heard whole); recordings are 2 fps samples with no audio track.
- Perception notices need a Jev key; notification triage is shadow only; "now playing" isn't built.
- Learning has no observed learned procedure yet; no measured memory benefit.
- Model providers' own refusals; provider latency dominates response time.
- The CLI doesn't show the live progress line.

---

## 7. Roadmap after launch

Decided or discussed, deferred (spec §6 and plan):
- **Security phase 2:** trust by author, not location (inbox, downloads and pages are outside even when local);
  following a link vs building one; per-task folder scope from the owner's words (closes the L2 residual); tagging
  memory written after outside content; outside code quarantined; links from outside content opened logged out;
  writes scoped to task folders; "authorize this app" pages treated as account security; sensitive-by-content files
  (incl. Indian ID formats); narrower trusted contacts; a delegation mode; send approvals bound to exact text.
- **Distribution:** Developer ID signing and notarization (no repackaging), auto-updates, in-app onboarding and
  settings, merging the voice helper into the main app, possibly downloading runtimes on first use to shrink the app.
- **Extension:** optional host permissions requested at pairing instead of `<all_urls>`.
- **Platform:** a Swift native layer; a Rust core was discussed (the 0.27 plan kept Python, D1).
- **Perception:** enabling notification triage after measured owner labels; always-on hearing only if the owner
  decides; "now playing" if feasible.
- **Other deferred items:** coverage checks against real reads for fan-out (P15), F9 thread matching, Jev's
  destructive classifier leaving shadow after measurement, whole-database encryption once a signed helper exists,
  embedding adoption after evaluation, after-launch code-health items.
- After the launch-week fixes, `astra_resume` becomes `main` and other branches are deleted (L14).

---

## 8. Glossary

- **Owner** — the person FERB works for; only their words carry authority.
- **Daemon** — FERB's background process (LaunchAgent) that plans, runs tools and records everything.
- **Notch app** — the macOS chat UI in the notch (`ui/mac/FERBNotch`).
- **Planner (orchestrator)** — the model turn that reads the message and context, answers directly or writes a task.
- **Worker** — the model run that carries out one task or plan step with the tools its identity allows.
- **Checker (verifier)** — the separate final check; observation-only tools, enforced by code.
- **Gateway** — the code every tool call passes through: validation, paths, locks, confirmations, recording,
  redaction.
- **Identity** — whose account a tool acts as: the owner's profiles or FERB's own accounts.
- **Ack** — the instant neutral acknowledgement (P25 phrase list).
- **Plan line / live line** — the one-line plan and the code-built current-step line in chat.
- **Episode** — the permanent record of one task.
- **Procedure / learned** — a reusable tool-call template from verified episodes; "learned" after three verified
  successes on two slot values.
- **Digest** — pending identity suggestions shown at most once a day for Confirm / Not right.
- **Jev** — an optional hosted typed-decision model (`ferb setup --jev`) that filters activity batches and decides
  whether FERB should consider them; its other questions run in shadow.
- **Shadow** — recorded and measured, with no effect on actions.
- **Notice** — FERB's chat message about something it observed, tagged "noticed".
- **Origin** — where content came from (owner, FERB, private, inferred, `external:…`); outside content is data,
  never instructions (MP2).
- **Code floor** — the code-enforced questions: payment, account security, permanent deletion, send to a new
  recipient.
- **S4 (sink rule)** — the check on private data going to an unnamed place after outside content was read.
- **Standing permission (D30)** — an exact send scope FERB states for one long task (or records with a schedule from
  the owner's typed words); the owner corrects it by replying; in-scope sends don't ask and are logged.
- **Sealed fill** — a password or code typed by the owner into hidden input and filled straight into the exact
  field; no model sees it.
- **Keychain handle** — a reference to a secret the owner chose to store; models see only the handle.
- **Escape hatch (D31)** — a one-time owner approval to cross one code boundary for one exact command.
- **P19 / unknown outcome** — an action that may or may not have happened; never retried without the owner's Yes.
- **Sandbox / egress proxy** — the macOS profile the shell runs under, and its only, recording route to the
  internet.
- **Recovery copy / snapshot** — FERB's pre-change file copy (7 days) and local APFS snapshot.
- **Disturbance sentinel** — the recorder of desktop changes during tasks.
- **Delegated conversation (F9)** — an approved, scoped conversation FERB carries on with hand-over.
- **Big job (D24)** — a large fan-out plan stated with time and cost estimates.
- **Test mode** — an allowlist that refuses send-class actions except to listed recipients (or `"SITE @ACCOUNT"` for
  a post) during supervised tests.
- **L-numbers** — launch-week decisions L1–L28 (2026-10-13/14); **D/P/S/MP/W** — 0.27 plan decisions; **F0–F10** —
  the 0.27 milestones.
