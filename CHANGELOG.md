# FERB version history

Versions use three numbers, `MAJOR.MINOR.PATCH`:

- **MAJOR** stays `0` until FERB leaves Block 1 (the personal, single-Mac version).
- **MINOR** goes up for a new capability (a build step, a unit, or a spike that changed how FERB works).
- **PATCH** goes up for fixes and smaller changes inside that capability.

`0.0.x` is the pre-release foundation, ending at the §21 safety suite. `PROGRESS.md` called that milestone "v0.1".
Each entry names the commits it covers, so `git show <hash>` takes you to the code. The current version is in
`pyproject.toml` and `ferb/__init__.py`.

## 0.27.0.dev1 launch week (L1–L28) — 2026-10-13 to 2026-10-14 — integrated, owner install and live checks pending

The fixes from the owner's first days of daily use, before the public launch on 2026-10-21. Decisions L1–L28 are in
`docs/superpowers/specs/2026-10-13-launch-week-fixes-design.md` and the plan's "Launch-week clarifications"; every
task had a fresh reviewer. Not installed and not live-tested yet (`docs/OWNER_ACCEPTANCE.md`, "0.27 launch week").
Commits: those after `6d613a4` on `astra_resume`, which became `main` on 2026-10-24 (L14: fast-forward; the other
branches deleted; the parked Safari spike kept as the tag `archive/spike-5-safari`).

**Live fix (owner's first live use, 2026-10-14):** hearing captured nothing on a real Mac. PyObjC returns the audio
stream description as a plain tuple, not an object with fields, so every buffer failed ("'tuple' object has no
attribute 'mFormatFlags'") and the model was told the media might be paused or muted. FERB now reads the tuple by
field order, and a capture error says it is FERB's problem, never the media's.

Fewer questions (L1–L5, L24):
- Shell and browser reach any site without asking. The proxy still records every host, and the full answer ends with
  "Sites contacted: …". This replaces the per-host (2026-10-10) and per-command (2026-10-11) shell questions; in the
  traced week they were 65 of 100 approvals, 46 in seven minutes for one research task. The owner's No to a host still
  holds for the shell. Accepted residual: after a malicious page, the shell could send a file somewhere (recorded).
- FERB asks only for money, account-security changes, permanent deletion and sending as the owner to someone new,
  unless a standing permission covers it; plus the private-data check (S4), escape-hatch commands and unknown-outcome
  retries. A named tool that isn't installed is asked about once.
- The private-data check no longer fires on FERB's own context words ("search", "playlist", "about-us"): only actual
  private values count, and FERB's own memory and history don't make a task "outside content".
- Bulk sends and folder deletes ask once, with a count, size and a sample.
- Big-job plans are stated in the plan line and start at once; reply to change them.
- Answers: say "yes" or answer in your own words; FERB routes it to the right question by id and asks "which one?"
  when it can't tell. A correction never records a No. Questions never expire while their task waits. The CLI no
  longer auto-answers the only open question.
- A command can reach a pinned local server (psql, redis-cli) directly; the kernel's reports record each connection.

Chat that keeps up (L6–L8, L20):
- The daemon acknowledges every message at once with a short phrase from the approved list, then shows the plan line
  as soon as the planner returns, then one live line with the current step and elapsed time.
- Every chat event names its request, so a late answer appears at the bottom as a reply quoting its question.
- Questions appear inline in the app while typing stays enabled; the paste dialog is gone; notices appear in chat.
- The app keeps only the daemon-redacted copy of what you sent, never the raw text or the unsent draft.
- The app steps aside for its own file picker, other apps' menus over the notch, and apps you bring forward under the
  pinned chat; it never toggles click-through.
- `ferb baseline` records the time from message to acknowledgement, plan line and answer.

Answers in your words (L11):
- Unfinished tasks get a short answer in plain words (what was delivered, what isn't confirmed, what you can do); the
  checker's findings stay under Details. No more "Verified." prefix: the app shows a check mark.
- A correction after an answer re-plans the same task with the new goal. A refused reminder or memory write keeps the
  reply and adds one plain line. A reminder you asked for earlier in the same chat is set. Steering says what changed.

Seeing and hearing (L12, L13, L26):
- The planner sees your front window with every message: app, title, browser tab and one redacted screenshot taken
  when you send. It can answer a question about your screen directly; the worker starts from the same picture.
- Browser windows, tab screenshots and recording frames now reach the model redacted instead of being refused:
  password and code fields are painted over, and an image is withheld only when masking fails. Extension 0.15.3 /
  protocol 14.
- FERB can hear an app during a task: `audio.listen`, recordings with a time-aligned transcript, and `browser.media`
  to play or unmute a video first. Transcription is on-device only; the sound is deleted once transcribed. FERB's
  voice helper runs only with your own `~/.ferb`, so no other process can point its microphone or transcripts
  elsewhere; a recording stopped midway is still finished once.

Perception in chat (L9, L10):
- Notices are chat messages tagged "noticed", not macOS banners (banners only when no FERB client is open).
- One moment gives one notice: FERB considers it once per app with your memory, the recent chat, running tasks and
  what it already told you, and says what happened and what it means; outside text is quoted with its origin.
- FERB no longer narrates its own clicks or reads its own banners back; no backlog at install; the seven-day purge also
  clears notice text derived from typed or clipboard text.

Planner context (L28):
- The planner uses each model's real context window instead of a hidden 32,768-token cap that squeezed out the
  situation, activity and memory on 85 of 87 turns. The situation, running tasks, questions, memory and today's
  activity always go in; chat fills the rest newest-first, with older chat summarized; the order is cache-friendly.
- Replays of real turns: message to plan line 11–17 s → 1–2 s on FERB's side; the reusable prompt prefix grew from
  about 8k to 54k tokens. Episode capture runs after the answer, in its own process.
- FERB's Codex model is `gpt-5.6-sol` (GPT-5.5 retired from Codex for ChatGPT sign-ins).

Broken things fixed (L16–L18, L21–L23):
- Learning: redaction no longer corrupts internal ids (35 task records had failed to save); the consolidator's
  schema works in strict mode; the hourly memory loop survives a failing step.
- Typed text is captured per field again, never from password fields (shown ones included), Secure Input or terminal
  apps. A choice answer (the 2FA "1") is no longer treated as a secret.
- A Codex account error always gets a reply naming it, scheduled runs included. Waits after the Mac sleeps end on
  time. A large folder inventory no longer freezes the daemon.

Packaging and setup (L15, L25, L27):
- The notch UI lives in this repository as `ui/`.
- One FERB.app in a DMG: the notch UI, the daemon's Python, Codex, the secret helper and the voice helper in one
  bundle, ad-hoc signed now with one signing step ready for Developer ID. The daemon runs as the agent app "FERB".
  FERB.app's own code is protected from FERB's tools and shell. Install from source still works.
- `ferb setup` asks every Chromium browser it finds to install FERB's Chrome Web Store extension where that is
  documented, registers the native host and reports per profile. The listing went live on 2026-10-20 and its id
  (`plbfkifcmgkgnlfgikdeaaglgncdhkma`) is built in (`07b2dd8`); `[browser] store_extension_id = ""` turns the request
  off and keeps the Load-unpacked steps. The listing carries 0.15.6, which this build needs, since 2026-10-22.
- The Trash: FERB moves items in and out only between folders it holds open, never moves something out after a
  mismatch, and FERB's shell can't write any Trash.
- Campaign path: a browser post declared as a send (naming the account it goes out from, no recipients for a public
  post) is checked like one, so standing permissions cover it. A schedule records a standing permission only for
  destinations you typed, each run gets exactly that, and cancelling or pausing the schedule ends it; a correction to
  a scheduled or resumed run never widens its scope and never moves it out of the cancel's reach. In test mode, allow
  a post as `"SITE @ACCOUNT"`.

Final review fixes (2026-10-14):
- The background "should I tell you?" check reads only the chat since FERB's last notice in that conversation, plus
  FERB's running summary of older chat (with your memory, running tasks, notices already told and the observations),
  instead of the whole conversation that fits the model's window (owner decision I2, 2026-10-14).
- A standing permission stated in a plan or a correction covers a send only when its site, account and recipients are
  in words you typed (this request, its corrections, your answers, or when you told FERB to remember them). Any other
  is stated as not covered, so a send to someone you didn't name asks, as for a schedule.
- A message that answers a question and also asks for something else ("the work one, and stop the other task")
  does both; before, everything but the answer was dropped silently.
- A question no longer raises a macOS banner while the app is connected and showing it; a banner, when one is sent,
  names the question without telling you to "open ferb chat".
- A notice is part of whichever conversation you then chat in (the app's, voice), so "what's that about?" works there,
  not only in the daily conversation.
- Scheduled runs and plans resumed after a restart release their per-request context (each planner block's full text)
  when they end, instead of keeping it in memory until the daemon restarts.
- The app's dictation is recognized on this Mac only, like push-to-talk: where on-device recognition isn't available
  for your language it says so instead of sending your voice to Apple's servers. The Speech Recognition prompt says so.
- Each task stores the conversation it was shown once, not twice, and a navigation's private-data check prepares it
  off the daemon's event loop.
- Codex's shipped model catalog is dumped at the daemon's start, in a worker thread before it serves; reading a
  model's window never runs Codex on the event loop.
- Cancelling or pausing a one-time schedule while its run is under way now withdraws that run's standing permission
  (before, the schedule already counted as done and couldn't be cancelled).
- The private-data check no longer asks when a link only carries a value that the same site already had in its own
  address in your data (opening the Instagram reel you have open; the live "analyze the insta post now" question).
  The same value going to another site, or also found elsewhere in your data, still asks.
- Workers report a CAPTCHA block only with the challenge or error the page shows; a reCAPTCHA or Turnstile footer is
  not a block, so the page's real message (an "application not found" error) reaches you instead of a foreground
  request.
- When FERB rewords a worker's question for you, it keeps the worker's reason (one short clause), not only the
  choices, so you can see it and correct it.

Fix wave 2 (2026-10-15, after the whole-codebase shippability review):
- One-time codes as providers send them are masked on every capture path (notifications, typed text, chat, transcripts,
  OCR): a 4–8 digit code with a code word a few words away on either side ("123456 is your Instagram code", "G-482913
  …", "Your Amazon code: 482-913"); before, 2 of 11 real formats were. A value next to a secret word is masked too
  ("password: sunshine2024", "my api key is sk_live_…"). Numbers with no code or secret word near them stay readable
  (owner decision C1). After the reviews: streamed output (shell output, replies, CRLF terminal output included) masks
  every code the text redactor masks, checked by a fuzz test, while number-only output (`seq`, id lists) and prompts
  with a number ("Delete 1234 files? [y/N] ") pass straight through; a bank's OTP after "Rs.", "a/c no." or "Ref. No."
  and a code after a bracketed sender ("[Swiggy] 482913 is your OTP") are masked; an email's code on its own line (under
  "Use this code to sign in", colon or not; in brackets, after a bullet, in markdown or HTML marks or in a quoted mail
  too, on screen as well; "Your code is: **482913**" on one line too) and every code in a backup, recovery or 2FA code
  list (blank lines, leading commas and item numbers such as "1. " allowed) are masked, while other "codes:" lists
  ("HTTP status codes: 200, 301") stay; a password on a line under its label is masked whole ("password for gmail:" over
  "s3cret pass 9"), and so is a value whose quote doesn't close on its line ("password: 'Tr0ub4dor&3"); no-break and
  other Unicode spaces read as spaces ("Code:&nbsp;482913", a card grouped by them); your own requests keep their
  numbers ("Pay 2500 to Rahul and verify", "PR 4821 is ready, please review the code"); address and coupon codes (PIN
  code, ZIP code, Pin/Zip code, promo code) stay readable (owner decision); your typed answer to a question that
  mentions a password (or "pwd", "passwd"), passcode or PIN is hidden whole (a choice you pick never is), and the code
  in your answer to a question about a code is masked. Streaming number-only output costs about a quarter of what it did
  after the earlier fixes (`seq`'s 29,000 lines read 1 KB at a time: about 0.4 s → 0.1 s; 0.05 s before the redaction
  fixes).
  Closing round (re-review 6): redaction no longer stalls the daemon on the inputs found. A line with a number, a long
  run of spaces and two more words made one pattern take exponential time when a shell read ended inside that line
  (FERB's own source, aligned comments, padded tables: past 20 s for one read, with everything waiting, stop included);
  so did runs of self-closing tags ("<br />", minified HTML: 22 hidden inputs, 2.9 s), and minified JSON with secret
  keys, long space-free tokens and repeated "eyJ" took quadratic time (256 KB of JSON: up to 46 s). These patterns and
  scans now grow in proportion to their input (the same JSON: 0.07 s). A test checks the shapes it builds (whitespace,
  markup and tags, digits, dotted and slashed runs, JSON with secret keys, unclosed quotes, repeated prefix words and
  "eyJ", key headers) by doubling them through every redaction pattern, `Redactor.text` and a stream read; other
  shapes are not covered. Also masked now: "my password is 'sunshine'", "Your code: <code>482913</code>", and in
  streamed output a code next to an HTML tag with spaces in its attributes or a tag split by a read, and a card followed
  directly by markup ("4111 1111 1111 1111**", "|…|", "<td>…</td>"); "Port <code>8080</code> is the default" stays
  readable. Known gaps: a quoted mail's password or backup codes with a ">"-only line under the label, a code under its
  label in HTML source with tag-only lines between, and text in angle brackets that reads as a tag ("<Your code is
  482913>") still show.
- A DMG install can answer: setup (and FERB.app's first-run window) asks "Sign in to ChatGPT for FERB now? [Y/n]"
  when FERB's own Codex has no sign-in and runs the browser sign-in; FERB.app's menu has **Sign in to Codex…**;
  `ferb doctor` fails (not warns) without it, since the daemon never uses your personal Codex; `--update` says how.
- A recurring schedule's run sees its previous run: when it ran, how it ended and its answer (with that answer's own
  origins), and `chat.history` lists every earlier run, so "notify me if it falls or improves" compares. Live: the
  daily Kaggle-rank run on Oct 15 found no earlier result and reported rank 205 as a first baseline instead of a
  59-place drop from 146.
- The private-data check's "the site already had it" rule matches the exact host (or a host under it), not the
  registrable domain: a token from your own app at `you.vercel.app` going to `someone.vercel.app` asks again
  (likewise `*.github.io`, `*.netlify.app`, `*.pages.dev`, …). The live Instagram reel case still opens.
- `browser.media` reports a video as playing only once its time moves, not as soon as play() was called (which clears
  "paused" before anything plays); it also reports `paused`. A media that never starts never reads as playing.
- The planner waits for your front window at most ScreenCaptureKit's own capture timeout (10 s), then answers
  without it and says so; a hung or very large app in front no longer delays every message without bound.
- "Don't worry, leave it, it works now, but if you find the reason, tell me" narrows a running task (stop changing
  things, keep diagnosing, report) instead of stopping it; the planner stops only when your words ask only to stop.
- A task you stop reports what its worker had found so far ("What I'd found before stopping (not checked): …")
  instead of only "Stopped, as you asked.".
- When the AI service can't be reached and the Mac has no network route at all, the reply says the Mac seems to be
  offline instead of "try again shortly". A login problem is never called offline.
- A one-time schedule can be cancelled or paused from the moment its last run starts, its planning included (before,
  "There's no active schedule…" for a moment, then the scope was granted); pausing it says it has no future runs
  instead of leaving it "paused" for good.
- An answer that comes with the planner's own reply is one message ("Got your answer. …"), not two; a message sent
  right after an answer that also stops, steers or starts something waits until that has registered.
- Screen-capture failures keep ScreenCaptureKit's error domain and code (and its stated reason) in the result, trace
  and journal, so "Failed to start stream due to audio/video capture failure" can be diagnosed; a minimized window
  is reported as minimized (FERB leaves it minimized); a hearing capture whose start timed out is stopped.
- Clipboard capture respects macOS 15.4+ pasteboard privacy: FERB reads what you copy only when macOS allows it
  without asking (FERB set to Allow in Paste from Other Apps), so it never raises a surprise alert; setup offers to let
  macOS ask once, and `ferb doctor` says when clipboard capture is off and how to allow it.
- The front tab's URL and title are read for Edge, Arc, Vivaldi, Chromium and Chrome's Beta/Dev/Canary too (not only
  Safari, Chrome and Brave), and a title with commas or quotes comes back whole.
- "Sites contacted" also lists the pages a task's own tabs loaded (search results it clicked, where a redirect or
  form took it, a popup it adopted), not only the addresses it asked to open.
- A reminder asked over a clarification ("remind me about the show on oct30" → "What time?" → "anytime" → "7pm") is
  set: a quote made of the owner's own messages in that request, joined, counts. "Anytime" no longer gets the
  question again: the planner picks a time from context (or your stored preference) and says which; and a reply never
  claims a reminder is set before FERB's own line says so.
- "What is this song?" is answered from the page only when the page names it; otherwise FERB listens on the tab you're
  looking at, and says plainly when it hears speech or music it can't identify (its hearing transcribes speech; it
  doesn't recognise music) instead of naming something the speaker mentioned.
- "Do this for all 15 items in parallel" becomes a plan with one step per item, never one worker told to be parallel;
  "parallel" is never a success condition, and a task never fails only because of how the work was split.
- "Are the agents working?" is answered from FERB's own record of the task (running, blocked, finished, the last
  worker report, step counts), never from what the planner imagines.
- A task's tabs are known by tab id: a tab it took over and then navigated elsewhere is still the same tab (with the
  page it held when taken over, stored redacted), so it never counts as missing; a held tab you close is listed as
  closed by you, and reopening it is a new tab.
- In a plan run, a blocked step shows the task as blocked in FERB's record. Each turn's record of finished tasks reads
  only the tasks recorded since this chat session started.

Fix wave 3 (2026-10-15, after the memory and learning review):
- Learning keeps going after the 30-day trail purge and after a forget. An episode card cites only its task's own
  sources (your message, its tool results and evidence, the procedure it used), not every chat entry and task its
  planner was shown. A forget leaves a tombstone (the ids it deleted, never their content): a card built on forgotten
  content is never written or rebuilt; a source removed by retention stays a link of unknown content. One forget no
  longer deletes the cards of unrelated tasks.
- Only your own words are saved as yours. "Remember that …" saves the whole sentence you typed it in (so a slice can't
  drop a "never" or an "if …"; only a line break or a pasted part ends that sentence, never a dot, "!" or "?", so
  "never email my prof. Zorblat" or "never! Share my links" stays whole, "Book my flight tomorrow.\nI prefer aisle
  seats" saves just "I prefer aisle seats" (a paste mid-sentence splits it too: "I don't [paste] like tea" saves
  "like tea"), and a saved memory can be longer than what the planner picked);
  FERB's rewording of it waits until you reply Yes to its question (in chat or the notch) before it
  reaches a prompt, and a quote like "the" no longer turns the planner's sentence into an owner rule. A preference in
  your own typed words joins your standing rules at once, whatever was on screen (your decision); pasted text never
  does. Unconfirmed hints read as unconfirmed to the planner.
- Forget goes further: FERB's "Remembered: …" line, its answer to the forgotten message, the lines of its replies that
  restate the item, and what a task given the item did with it (its answer, outcome, saved plan, permanent record,
  "while you were away" result, its tool calls and turns, and its episode card when that restates the item) go too,
  from the chat and from FERB's stored records; a task shown only something built on the item's source that doesn't
  mention it isn't touched; a question FERB was still asking about the item closes (answering "Keep this?" never
  closes your own "Forget for good?", which waits until the item is deleted); the scrub runs off the event loop and
  your next message waits for it. It removes lines that hold the item's words, as recorded or as FERB's questions
  showed them, not every paraphrase; in FERB's stored turns it matches whole words, so "Gmail" never takes a piece of
  another word or a one-word line elsewhere. You can forget from chat or the notch ("forget my test lab colour"): FERB
  names everything that goes and deletes only after your Yes. Forgetting a learned item (a lesson, a procedure) also
  removes FERB's naming of it in its question; what each of its versions states, from the records of the tasks, chat
  turns, background checks, notices and model runs that were shown it (an identity statement: every task and model run
  since you set it), as whole words in stored turns; and a lesson's or identity's own text (two words or more) from
  any FERB reply or notice that repeated it word for word. A trigger or a procedure's name never touches a reply it
  wasn't shown in, and another task with the same goal is never touched; a procedure's steps shown
  to a worker, records deleted only because they were built on it, a lesson's scope labels and an entity summary's
  text (unless recorded exactly) can survive. A failure to record that
  a task read a memory is noted in the journal and never fails the read.
- The sleep service works on what changed: a card is read again only when its inputs change; episodes go to the model
  grouped by the kind of task they were, one call per kind, fitted to the model's window (learning by intent, L29,
  below; this wave first grouped them by the tools and apps they used); a failed job waits for a change or `ferb
  memory consolidate`, and a failed distill day for its evidence to change, instead of retrying every
  hour. The day rollups run off the event loop and never once a task has started; forget no longer rebuilds search
  indexes on it.
- A planner's procedure id that isn't a current procedure is noted in the trace and ignored; the task runs.
- "Not right" on a suggestion stops it coming back; two slot values must be two real values; an app update sends a
  learned procedure back to candidate instead of marking it suspect for good; a procedure receipt checks its ids
  first; local embeddings say they need the optional memory extra before downloading anything.
- FERB keeps its own pinned tokenizer copy under its home (setup puts it there; `ferb doctor` checks it), so token
  counts no longer fall back to a byte ceiling when macOS cleans the temporary folder.

Fix wave 5 (2026-10-20, why background learning never worked live; L28, D32):
- Learning failed on every job since 2026-10-11 (226 of 226 on the owner's Mac, 210 as "Codex could not complete
  turn/start"). Codex refuses a turn whose text holds more than 1,048,576 characters (its `MAX_USER_INPUT_TEXT_CHARS`,
  openai/codex rust-v0.157.1), and a consolidation call sent about 97.6 million. FERB now knows that cap from Codex's
  source and fits every call to it as well as to the model's token window: the planner, workers and checker keep
  chat and optional context only while both fit, the learner and the running chat summary split their calls to fit,
  and anything that must go whole but can't is refused before sending instead of being rejected by Codex: the reply
  says FERB didn't send it and how far over the limit it was, and the trace names each part's size.
- A Codex failure now says what Codex said, e.g. "Input exceeds the maximum length of 1048576 characters. (code
  -32602; input_too_large, 1100000 characters sent)", redacted, never with what FERB sent: in learning jobs, spans
  and `ferb trace`, and in the reply when it happens inside a running turn.
- Every learning model call is a `model.learning` span with its size, outcome and error (a stop is "cancelled", not a
  failure). `ferb stats` counts learning calls and failures, groups the errors and names the latest failed one;
  `ferb trace distill-2026-10-19` (or a `sleep-…` batch id) shows it. `ferb doctor` names the character cap.
  Consolidation's model usage is recorded as consolidation, not distillation.
- An error Codex says it is retrying itself (a dropped stream, for example) no longer stops the task or learning call:
  it is recorded in the trace and the run goes on. Retries alone don't keep a task alive: Codex retries a lost
  connection without end, so a worker that only retries for 3 minutes is stopped, and the task says the Mac seems
  offline or quotes Codex's last retry.

Fix wave 4 — front window (2026-10-19, after the owner's visa question about a Greenhouse internship page was answered
with "Which internship is this?"; not installed or live-tested):
- The front window's URL and tab title are those of the browser window FERB captured. AppleScript's `front window` is
  the browser's own frontmost window, which was another Brave window (a Kaggle discussion). FERB now lists every
  window and takes the one at the captured window's place, ties broken by its title; when it can't tell, it sends no
  URL or tab title and says why.
- No window picture is refused for where its secret fields are (owner: "no screenshots should be refused; go with
  redacted stuff"). When neither the browser extension nor Accessibility can read the page, or a field has no box, or
  Accessibility can't read the window, the picture goes to the model masked by on-device text recognition only, with
  a note saying so and why (which extension or Accessibility step failed); a field that moved during the capture is
  painted where it was before and after, and whatever one look located stays painted when the other look can't be
  used. Only a picture the masks can't be painted on is withheld. Residual: where fields can't be located, a secret
  the OCR doesn't recognise as one can show (text recognition reads a row of password bullets almost never, so a
  shown-as-dots field is not something it covers).
- With two windows at the same place (maximized), Accessibility's window is the one with the captured window's
  title, confirmed against the window server's title at that moment (a retitle can't make FERB read the other
  window); a browser window is read again for ~2 s while its page area hasn't appeared yet. Brave and Chrome ignore
  `AXManualAccessibility` (it only helps Electron apps).
- A browser capture asks the extension while Accessibility reads the window, and doesn't ask again after an ask
  nobody answered, so even its slowest case (~6–8 s) fits inside the planner's 10 s wait for the front window; it
  could take 12 s, and the message then went on with no picture, URL or title.
- Extension 0.15.6 (protocol 17): the extension tells same-place windows apart by title and names the matched window's
  tab, which the front window now uses (AppleScript stays the fallback, and its "several windows could be it" stands
  over the extension's match); the block says where the URL came from (`tab_source`). An answer from another page's
  window (a paired profile's window behind an unpaired capture) is rejected rather than painted over the wrong
  picture. A task tab whose page the extension can't script (a browser error page) is captured instead of refused,
  `browser.screenshot` included: painted from Accessibility when it is the tab its window shows, masked by text
  recognition, with a note saying what was painted. A page the debugger can't attach to (the Chrome Web Store) gives
  a picture of the whole browser window when its tab is in front and its window on screen; otherwise it is withheld
  with the reason. Captures of a page scripts can't reach keep the debugger attached as it is (no re-attach per
  capture, so a recording and a screenshot of it don't collide and a held download stays held), and a tab's input
  readiness failing never refuses a capture: the result says so.

Fix wave 6b — the checker starts with what the worker did (2026-10-21, after the owner's "low power mode + loud alert
at 10% battery" took two minutes of work and then three check rounds with about 50 evidence reads; not installed or
live-tested):
- The final check now starts with every tool call the worker made and its result, in order, every attempt included
  (a re-check after a repair gets both). Before, it had only the report and paged through the records one read at a
  time. A long result shows its first 12,000 characters (one evidence read's page) and where the rest starts; when
  the check's window can't hold everything, what was done stays first, then the newest observations, and each one
  left out is listed with its record id in its place, or, past a few thousand calls, counted in a note naming the
  first and last of a run, so the check's input always fits. Results stay marked as outside data and keep their
  origin; they pass the same redaction as every evidence read, so a code you sealed mid-task is masked for the check
  too. A record shows its result before a long input. In a scripted complete check of 53 recorded calls, evidence
  reads went from 62 to 6.
- A check whose AI service reports its context window overflowed (or that FERB refuses as too large to send) runs
  once more without the recorded calls, as checks ran before. That covers Codex, OpenAI and Anthropic (including its
  "prompt is too long" rejection); Google gives no recognisable signal, so a Google checker still fails there. The
  trace shows the rerun as the same check round, and the worker keeps its "verified in step N".
- A page of a recorded call never cuts a value FERB knows as a secret in two, in the check's context or an evidence
  read: the page ends before it, so it is masked whole.

Fix wave 6c (2026-10-21, "tell me when …" survives restarts and updates; not installed or live-tested):
- "Tell me when …" and "alert me if …" about something outside FERB's own work (a file, the battery, a price, a page)
  are watches: schedules, so they survive restarts and `ferb setup --update`. On 2026-10-15 "turn on Low Power Mode and
  alert me loudly at 10% battery" put the alert in a shell loop inside the task, which died on the next update. The
  planner now makes the work asked for now a task and the alert a watch in the same turn, and never waits for a
  condition with a loop or sleep inside a task. "Tell me when the agent you started is done" stays the task's own
  monitoring.
- A watch either ends once it happens or keeps watching; the scheduling line and `ferb schedules` say which. Each
  check's report of the condition is judged by its final check like any condition. A one-shot watch stops itself after
  a confirmed check finds its condition met (journaled `schedule.met`, its recorded send scope withdrawn as a cancel
  does), and that answer says FERB stopped checking; an unconfirmed or failed check never ends it.
- You're told once each time the condition becomes true, judged against the last check that reported it, which is
  stored with the watch (a failed check in between doesn't make it tell again; an unconfirmed alert counts). A check
  succeeds by checking and reporting; a condition that doesn't hold yet is not a failed task. A met condition comes as a "FERB alert"
  notification, as a reminder does, even with the app open or while you're away. A check with nothing new to tell is
  recorded but reaches neither the app nor `ferb chat`, and no client lists it as running. Checks run on the
  schedule's interval (every 15 minutes or more); there is no battery or file event.

Fix wave 6a — forgets while FERB is working (2026-10-21; not installed or live-tested):
- A forget no longer wipes the output of a step that finishes while its clean-up runs, and no longer misses that
  step's output. The clean-up runs beside the tasks still working; it read a step's record, worked out the new text,
  and wrote the whole record back, so a step finishing in between lost its result (the record kept the empty output
  it had when read), and a step finishing after it was read kept the forgotten words. Each record is now written only
  while it still holds what was read (otherwise it is read and cleaned again), and the clean-up goes over the steps
  again while one that was running when it started finished or changed meanwhile, until a pass finds none. The same
  holds for a task's outcome, a notice's text and a chat entry the clean-up rewrites (a chat entry is replaced in one
  step, so one the notice purge deletes meanwhile stays deleted), and for the seven-day notice purge, which now clears
  the notice text it actually replaced; recording a running step's measured input never brings forgotten words back.
- A task or chat turn still running when you forget an item: when it ends, the clean-up runs again with the same
  rules, so what it recorded afterwards (later steps, its answer and "while you were away" result, a chat reply) loses
  the item too, and quitting FERB waits for that; your next message waits for it once the task has answered. That
  second clean-up touches only the work that was running at the forget: if you state the item again, start a new
  task, or have a finished task carry on (a continue, retry or correction), nothing of it is changed; a task that a
  message still being planned at the forget starts afterwards is cleaned whole (a forgotten identity note included),
  and a correction sent just as the task finishes waits for the clean-up before it picks the task up. After a crash
  before such a task ends, what it recorded since the forget can still hold the item: FERB keeps the pending clean-up
  in memory only (for as long as the task runs, which can be days if it waits on you), so the forgotten words are
  never written down.

Fix wave 7 — FERB says at once when it loses its AI service (2026-10-21; owner: "it should be instant"; not installed
or live-tested):
- When Codex starts retrying a dropped connection during work you're waiting on (a message being planned, a task's
  worker or check), the live line says so the moment it happens: "Your Mac seems to be offline: I'll carry on when
  it's back." when the Mac has no network route, otherwise "Lost the connection to the AI service I use (codex); it's
  retrying and the task carries on." with Codex's own words (in a plain chat reply: "…it's retrying, and I'll answer
  when it's back."). Before, you learned only if the run later stalled or
  failed. Further retries stay quiet; when the run moves again, one line says it reconnected, and the task goes on.
  `ferb chat` prints both lines. A worker or check that only retries still stops after 3 minutes, as before.
- Background work (scheduled and watch checks, learning, the attention check) records the retry in its trace and tells
  no one; a scheduled run's failure still reaches you as before.

Learning by intent (L29, 2026-10-21; owner: "just have some connection to the tools used for the previous learned
things so that model gets faster with steps"; not installed or live-tested):
- FERB now learns the tasks you repeat. Before, runs were grouped by the exact tools and apps they used, so "find me the
  cheapest egg curry with some breads", asked three times, fell into three groups and nothing was ever learned. Now
  each finished task gets a label saying what kind of task it was and what changes between runs (the dish), written by
  the learning model; code only checks the label's shape and value types, never whether two tasks are the same. A
  task read again keeps its label, and forgetting a task removes only its label.
- Once a kind of task has three verified runs (your "counter 3"), one learning call gets its runs, failed ones too (as
  many as fit the model's window, each whole), with its procedure, its lessons and what you turned down, and writes
  the procedure: its tool steps with blanks for what changes, plus lessons from what failed. Every new run of that
  kind is learned from again. A step only some runs needed is marked optional instead of blocking the procedure. A
  revision is advised to keep only the example runs it needs, so the examples don't pile up until they no longer fit;
  code refuses one that cites a run you marked failed. A lesson you wrote or edited is never rewritten by the model: a
  model lesson with the same scope and trigger is refused (the prompt also tells the model never to restate it).
- A revision is still the same procedure. Every run it was credited with keeps counting toward "learned" (three
  verified runs with two different values), whatever examples the revision keeps, except the runs of a version you
  rolled back; two failed runs in a row make it suspect across revisions. Your confirm carries over to a revision
  whose steps built on outside content were all steps of the version you confirmed by hand (a confirm that carried
  over never counts as yours); a revision that adds such a step waits for your confirm again: `ferb memory
  consolidate` names it under `requarantined`, and when the hourly pass revised it, `ferb timeline` shows
  `memory.procedure_requarantined`. A proposal identical to a version that exists changes nothing (`unchanged`).
- The planner sees every procedure FERB can use by its kind of task (name, description, what changes), most recently
  used first and fitted to its window instead of matched by your words, and picks one by meaning with this request's
  values (dish = rajma). If any value is missing or of the wrong type, the values are noted in the task's trace and
  left out, and the procedure is used with its blanks.
- The worker starts from that procedure's steps with this request's values filled in, its lessons with your ratings,
  and the exact tool calls of the latest verified runs (newest first; the oldest give way first when space is short),
  so it reuses what worked instead of finding the tools again. A learned procedure is the fast path (follow the steps,
  check every expected result, full reasoning on any mismatch); a candidate is evidence only; neither grants
  permission. Only the worker gets the earlier runs: the checker judges this task's own calls.
- A procedure waits for your confirm (quarantined) when a step's destination or target came only from outside content
  (a page or message): a recipient or contact, the site a send goes to when it names no recipient, an account, an
  amount, or a path (a file written or moved, a command's working folder). Since learning was built (F8), the card
  reader read the gateway's record of where each target came from in the wrong vocabulary, so every step with a target
  (a recipient, a file, a site) counted as outside, even one you typed, and nearly every procedure would have waited.
  Now a target you typed or answered is yours, one found only in a page or message is outside, and one found in neither
  (FERB's or the model's choice) is not; cards built before are read again once, from their stored provenance when their
  trace has aged out. The site a step only ran on, and a step of a kind that records no target provenance, never hold a
  procedure (plan 2026-10-23 Task 2, "Browser procedures without a hold" below).
- Learning records stay valid when secrets are masked in them, so a run whose text holds a card-like number or a
  labelled secret is still saved; one that can't be saved without a secret showing is refused, never stored.
- `ferb memory consolidate` lists each label and each kind of task's call; `ferb trace label-<card id>` and `ferb trace
  <a procedure's batch_id>` show the calls; the planner's choice and the worker's input are in `ferb trace TASK
  --full`.

Accepted residuals (L29):
- A procedure you forgot is never learned again for that kind of task, and one you turned down ("Not right") blocks
  any procedure for it (the M3 rule).
- Forgetting the run whose label first named a kind of task leaves that name and description in its other runs'
  labels and the planner's line (they describe the kind of task, not that run).
- In a very tight worker window the earlier runs may show only as markers or one counted note; some odd text shapes
  (a quoted "Password:" label inside a sentence, a value split across strings) can be masked more, or matched less,
  than intended (ARCHITECTURE, "Redaction stays JSON").
- A procedure whose slot is named like a credential (a token, a PIN) never becomes learned: those values are masked
  when stored, so its runs count as one value.
- "Found in neither" includes an outside value code can't match: one a page gave in another form (digits typed from a
  number written in words), read from a picture, or recalled from memory. It counts as chosen, not outside, as FERB's
  confirmations already read it; a send to a destination FERB doesn't know still asks each time. Treating it as
  outside when the run read outside content is your decision.
- A new intent name that is a known intent the labeling call wasn't shown (its list didn't fit the window), with
  other blanks, is refused; the job's error names that intent, and the card waits until it changes or `ferb memory
  consolidate` runs the call again. Rare.
- A procedure whose line doesn't fit the planner's window (many procedures, a long chat on a small-window runtime:
  chat is fitted before the procedure lines) is only counted in a note on that turn and can't be chosen.
- The procedure block a worker gets is marked as data in its header, not wrapped in FERB's untrusted-data wrapper; its
  model-written fields are JSON-encoded and the code-chosen mode comes first.

Database clean-out (L28, C1; plan 2026-10-23 Task 1; owner 2026-10-23: "clean out yes, so we dont face performance
issues with ferb again"; not installed or live-tested):
- The owner's installed FERB had an 8.1 GB database, 6.5 GB of it in `learning_sources` and its two indexes: 22.6M
  source rows on 438 versions of the run cards of 85 tasks, about 52k rows a version (the largest 58.8k, 57,335 of
  them `trail:` refs). The card composer before memory review C1 (`code:evidence-card/1`, the installed one; fixed in
  e0e344b) copied into every card the refs of every block its planner was shown (the day's activity, the
  conversation), and every earlier version's. The current composer never writes them, but the old versions kept them.
- The next `ferb setup --update` cleans the database once. After installing, it stops FERB, keeps on every version of
  each card only the source rows the current card rule gives it (the task's episode, the owner's request, the
  procedure it used and its own actions' sources), and compacts the file when the disk has room for a copy (free
  space at least the database plus its WAL). It then starts FERB again and prints "Cleaned FERB's database: N unused
  rows removed; 8.1 GB → x GB". A card whose trace has aged out is left as it is, since its lineage can't be read
  again. No edge is added and no other record is touched. On a synthetic store of the measured shape on the owner's
  Mac (23.3M rows, 9.0 GB), cleaning took 92 s and compacting 17 s, leaving 1.95 GB (1.6 GB of it stand-in for the
  rest of the store). The deletes run without the source-ref index, which is made again afterwards: with it, each
  card version's delete rewrote about 50k random index pages, and the clean-out projected to 47 minutes. Setup prints
  how far it is at each tenth.
- It never runs while a daemon answers (`ferb daemon run` in a terminal: stop it first), nor while the daemon launchd
  stopped is still exiting: setup waits up to 10 s for that process to end, else says so and leaves the clean-out to
  the next update. Each card version is cleaned in its own transaction. A crash, power loss, Ctrl-C or FERB.app
  quitting leaves every version either clean or as it was; FERB is started again, and the next update finishes the
  job. If a power loss cuts it off before the index is made again, FERB's next start makes it (each start makes it
  when it is missing). After a stop, setup says it is making that index again before it does (a minute on a big
  store). The journal records `store.cards_cleaned` with the counts once the rows
  are removed, and `store.compacted` with the sizes once the file is compacted. With too little free disk the update
  removes the rows but doesn't compact, says why, and the next update compacts without cleaning again. `ferb doctor`
  shows the database's size and what is left: the clean-out, or only the compaction. A store that won't open is
  reported, and the update goes on.
- After it, forgetting an activity row a task's planner was only shown no longer touches that task's card (the
  current rule since C1). Forgetting one its actions used still forgets the card and everything built on it.

Perception's cost (P1/D10, perf plan 2026-10-23; owner: "i dont want my mac hot al the time"; not installed or
live-tested):
- The perception batcher no longer re-reads every unsent trail row, re-encodes them and counts their tokens on each
  wake (once a second and on every new event). On your installed daemon that kept the main thread about 93% busy. It
  now keeps the unsent events in memory with each one's JSON and a running token count, and reads only rows newer than
  the last it saw. It sleeps until an event arrives, Jev changes, the oldest unsent event reaches `sense.batch_age_s`,
  or FERB stops: no 1 s timer. Migration 0044 counts trail rows deleted (retention, forget) or rewritten (the
  seven-day text expiry), so the events it holds are read again then.
- Every decision is unchanged: when it flushes and why (interrupt, tokens, age), which events go in which batch, the
  excerpts, how the notification context is trimmed, receipts, the install floor, and what happens while Jev is
  unavailable or comes back. EVENT_BUDGET (10000) and the age keep their values and meaning, and token counts stay
  exact: `count_tokens` adds up 8192-character slices, so only the last slice is counted again. The notification
  trimming counts the whole context only where a proven lower bound can't tell it is still over. With a huge message
  or rule (no ", " to cut at, or over 8192 characters) it can still count every step whole, as before, never more.
  Tests drive the old module (kept verbatim in `tests/support/batches_reference.py`) and the new one side by side on
  seeded streams.
- Measured on a synthetic trail of 400,000 rows with 1,200 unsent events (0.32 MB of JSON):
  - A wake took 100 ms. It now takes 0.005 ms with nothing new, and about 0.2 ms with one new event (0.34 ms
    including recording it).
  - Over 60 s with no events, the loop used 6.25 s of CPU (54 wakes, 10.4%). It now uses none (0 wakes).
  - Over 60 s at 150 events a minute (the rate measured on your Mac), it used 19.9 s (32.9%). It now uses 0.39 s
    (0.6%, including recording the events).
  - Batching the 1,200 events for Jev took 621 ms; it now takes 196 ms.
  - The first wake after a start reads every unsent row once (107 ms).
- An age flush now happens at the deadline, not at the first 1 s tick after it. A changed `sense.batch_age_s`, or an
  unsent row deleted or rewritten, takes effect at the next wake (the next event, a Jev change or the deadline)
  instead of within a second.

Browser procedures without a hold (L29, MP2, L2; plan 2026-10-23 Task 2; owner 2026-10-23: "i dont want that annoying
rec again, cant we just leave browser thigns as it is … make it just bare minimum so that our loop also does not
break"; not installed or live-tested):
- A procedure learned from browser runs no longer waits for `ferb memory confirm`. Two things held nearly every one.
  Opening or navigating to a site, like every step of a reversible kind (opening a site, app or tab, focusing a window,
  moving to the Trash, a timer, a recovery point, stopping a command), records no target provenance, and that counted
  as outside content. And typing or clicking on a page records the page's site, which the page FERB read names
  itself, so it counted as found only in outside content. Now a step of a kind the gateway records no provenance for
  (a reversible kind with no declared purpose: the gateway's own rule, `policy/rules.py` REVERSIBLE) is FERB's own
  choice, and the site a step ran on counts only when it is where the step sends (below): under L2 FERB reaches any
  site without asking. Only the card reader changed (`learning.outside_action` and `outside_targets`, with
  `confirm.site_is_destination`, the confirmations' own rule for when a site is a destination, now shared: they ask
  what they asked before); the gateway records what it recorded before.
- Still held until you confirm (SQ7: a step whose destination or target came from outside content):
  - a send to a recipient or contact found only in a page or message, such as a WhatsApp message to a number only an
    email gave;
  - a send whose only destination is a site only outside content named: a post, or a click or form input declared to
    send that names no recipient, on a site only a page named. FERB's confirmations treat that site as the
    destination too, and the card now keeps the step's declared purpose so it can tell;
  - an account, an amount, or a path (a file written or moved, a command's working folder) found only in outside
    content, such as a send from an account only an email named.
  Moving something to the Trash records no provenance and never holds a procedure (it can be undone, S1). A send,
  typing, click or other step of a kind the gateway records provenance for, recorded without it (before F3,
  2026-10-08, or a damaged record), still holds its procedure, so an old send never loses its hold. A target you typed
  or answered is yours, and one found in neither is not outside, as before. Your confirm carries over to a revision
  only when every such step was in the version you confirmed by hand, as before.
- The next learning pass reads every card again once (composer `code:evidence-card/4`); each keeps its kind-of-task
  label without a model call. Each kind of task with a procedure (or three verified runs) then gets one learning call
  to revise its procedure from the cards read again; the revision of a procedure held only by its site or its
  navigation is a candidate, or learned once its runs qualify. A card whose trace has aged out keeps the source rows
  it had, and its rebuild copies them once onto the new version. On the owner's store every card still had its trace
  on 2026-10-23 and setup cleans the database before FERB starts, so none of the old bloated rows are copied, provided
  the update runs before the oldest card's trace ages out (2026-11-07, at 30 days' trace retention). The launch agent
  setup installs has `RunAtLoad` (`ferb/device/install.py`): launchd starts FERB at each login while its plist is in
  `~/Library/LaunchAgents`, and whenever FERB runs, its retention deletes traces older than 30 days.
- Your earlier choice of recording provenance for the reversible kinds (more friction against memory poisoning) is
  moot: those steps no longer hold a procedure.
- Accepted with it: a procedure can learn navigating, typing and clicking steps on a site only a page named (a page
  that sent FERB elsewhere); a send there that names no recipient still holds it, as does any recipient, contact,
  account, amount or path only outside content gave. A click declared to pay or to delete is not a send: on such a
  site it holds a procedure only through an amount, account or path from outside content, while FERB still asks
  before every payment and once per page before a delete.

FERB's resource log (L30, perf plan 2026-10-23 Task 4; owner 2026-10-23: "a some kinda report for some days of use that
notes it down"; retention: "30 days"; not installed or live-tested):
- Every 5 minutes the daemon writes one row of numbers to its database: its CPU and memory (footprint and lifetime
  peak, as Activity Monitor shows them); the CPU, memory and number of the processes it started and theirs; FERB.app's
  CPU and memory; the database's size; and that window's work: activity events by source, perception wakes and
  batches by reason, Jev calls asked and not sampled, model runs and tool calls, tasks running, and the three busiest
  processes it started (program name, redacted, and CPU seconds). Never content, arguments or paths. Stopping FERB
  writes the minutes since the last row. Rows are kept `[retention] resource_days` (30), purged in the hourly pass.
- `ferb stats` ends with a "Resources" section: a line for each of the last `--days` local days, today included
  (hours covered, the daemon's CPU average and busiest window with its time as a share of one core, memory average and
  peak, the other processes' and FERB.app's CPU, the database's size), then the period's five busiest windows with
  their work counts. It reads the database, so it works with FERB stopped; against a store from before this update it
  says the log starts after the next `ferb setup --update`.
- Its own cost: each count reads only the rows added since the last sample. On a store with a 400,000-row trail and
  750 new events a window, one sample takes about 3 ms (most of it finding FERB.app's processes among about 740).
- Apps FERB opens with `open` belong to macOS's launchd, not to FERB, so they aren't in its numbers. A failed sample is
  journaled (`resources.failed`) and the next row covers its minutes too; a failure never stops FERB, its retention pass
  or its shutdown. The hourly retention loop now goes on the same way when even its own `retention.failed` entry can't
  be written (before, it stopped until FERB restarted). A command a long-running agent (Codex) runs counts in the
  window it ends. A day's peak in `ferb stats` is its window with the most CPU seconds.
- Residuals: forgetting something doesn't remove a program name from these rows (a script named after it, say); they
  age out after `resource_days`. After retention deletes the newest traces, a few new trace rows can go uncounted for
  one window.

After the owner's update (2026-10-24):
- FERB's cursor (L33; spec `docs/superpowers/specs/2026-10-24-ferb-cursor-design.md`; owner: "also add a cursor and good
  looking apple design principle cursor", "yeah works", "reviewd. start."): before an action the gateway asks the tool
  for the element's screen rectangle (0.5 s limit) and publishes the telemetry topic `cursor.point`; the notch app draws
  a click-through pointer that glides there and fades after 1.5 s idle (Reduce Motion: fade only; switch "Show FERB's
  pointer", on by default). Native taps report the Accessibility rect (screenshot clicks an 8x8 pt rect); menus and
  drags report none. Extension 0.15.9 (protocol 17, minimum 0.15.6) adds optional snapshot facts so the daemon predicts
  the point after the scroll-to-centre; fixed or sticky elements, inner scroll boxes, sideways scrolling, sub-frames,
  pinch zoom, FERB's minimized window and refused acts get no pointer. Real fix found on the way: `scrollIntoView` and
  `scrollBy` are instant, since smooth-scroll pages made real input aim at pre-scroll spots. No ripple or shimmer: the
  owner leaves decoration to Codex. Checkout validation: pytest 4426 passed, 1 skipped, 77 deselected; mypy ferb/core clean; Swift 231 tests; live checks in
  `docs/OWNER_ACCEPTANCE.md`.
- FERB's cursor, final review fixes (L33, 183244b): a window behind another window at the spot gets no pointer (the
  frontmost normal window there must be the target); native points read the element's Accessibility frame at act
  time, and a screenshot click whose window moved points nowhere; native acts the driver refuses before input, and
  keys (they go to the app's focus), get none; a typing point is clipped to the part of the field the page shows.
  `cursor.point` now names the step's root task (`task_id`) and the acting worker (`worker_id`), so a task keeps one
  pointer and one shade across its steps and its result ends it (they were keyed by the step's task id, never ended
  and took a new shade per step).
- Loops (L31; spec `docs/superpowers/specs/2026-10-24-loops-design.md`; owner: "there should be general capabilities
  like loop, or long running task so that it can use them here. not a per task addition everytime."): the planner can
  make a `loop` step for work that must be repeated or kept true ("keep this tab playing until I say stop", "loop
  until the PR's checks pass"). Each round is an ordinary worker run and check under one task id
  `<tree>.loop.<step>` (a `loop.round` span); the round names one exact field of a probe-safe observation tool and
  code reads it every 10 seconds with no model, starting a round when it trips, on a plan event, on the owner's steer
  or after 30 minutes. A loop with no `end_when` ends only by the owner's stop; `finished` needs observed evidence and
  the checker. Churn (3 rounds that didn't last within 10 minutes) backs off from 2 minutes, doubling to an hour, and
  tells the owner once; a pause comes only from `blocked`, `needs_user` or an unknown outcome. The loop state
  (`loop_state`, migration 0047) survives restarts quietly. The notch shows "Keeping: … · fixed N times · last X min
  ago" with Stop; `chat.status` lists `loops`. `ferb trace` shows `loop.round` and `loop.probe_read` spans. Not
  installed; live checks pending (`docs/OWNER_ACCEPTANCE.md`).
- Loops fix wave (L31, after the final review): a probe named as the runtime shows it (`mcp__ferb__shell_read`) now
  resolves, and a name two tools share is refused instead of guessed; `browser.read` is no longer a probe (it can't
  change between rounds); a probe snapshot leaves a tab's new downloads and tabs for the next round; each round's
  checker tabs and processes go after its check; a restart while a probe is armed no longer repeats a finished round;
  an unchanged probe read writes nothing; a forget removes the item from a loop's stored round results, note and
  baseline; the planner's task status shows each loop's state and a paused loop's question, so a reply resumes it; the
  notch no longer says "Working" while a loop waits between rounds; and a plan that FERB's shutdown suspends no longer
  records "I couldn't finish that" (its resumed run records the real end).
- A profile that already had the unpacked copy takes the Web Store request too, so it can hold both copies. Setup's
  report and `ferb browsers pair` now use the copy that is turned on (the unpacked one when both are). Before, with the
  unpacked copy turned off and the store copy on, setup said the profile's extension was waiting to be turned on, and
  pairing opened the turned-off copy's page and timed out.
- FERB's panel no longer vanishes when another app's window or menu needs the space (owner: "take it one layer behind
  rather than closing it back"). It steps one layer back and stays on screen: just behind the app's window under the
  pinned chat, or under the menu, popover or clipboard window over it. A screenshot shows it, and a clipboard app's
  paste into its input is visible. Click it (or hover the notch, or use the shortcut) to bring it to the front;
  clicking the other app sends it back. While it sits back it is an ordinary window, so a desktop swipe moves it like
  one. The paperclip opens one file picker at a time, and the panel is never put over its own picker. The owner
  checked it live: "ok it works".
- Chat (owner, 2026-10-24):
  - The quick reply ("On it now.") shows about 0.75 s after you send, as a small bubble on the plan line it introduces.
  - Steps such as "opening www.instagram.com · 9s" no longer show under the plan; the "N active tasks" row stays, and
    the steps are in View tasks › Inspect. The AI service's offline and reconnected lines still show there.
  - Command-C (and X, A) now work on text selected in the chat. The Edit menu's shortcuts never reached FERB's panel,
    because FERB never becomes the active app.
- The window FERB records when you send a message is now the front app's main window, from Accessibility. Before, it
  was whatever window of that app was on top. Owner's case 2026-10-24: a thin Brave pop-up strip was on top when you
  sent "play this tab". FERB shot the strip and got no link, then played a different Instagram tab twice.
- A follow-up that carries a task on is judged on what its own run did. The calls of the runs that answered your
  earlier messages are labelled as such, and the checker is told when this message's run began. Before, "keep it
  running" failed for the first run's mistake, though that run had already been checked.
- Pairing is automatic in Chrome and Brave (owner: "yes build", after OpenClaw's extension "pairs on its first native
  call").
  - **When:** an unpaired FERB copy says hello, or you switch to the browser while one is connected.
  - **Which profile:** the one you're in. The browser must be the frontmost app and the profile its last used, read
    twice 12 s apart because Chromium writes it about 10 s late, and the front app is checked again just before the
    page goes. FERB must be turned on there and the profile not yet paired.
  - **What you see:** a blank pairing page opens there and closes itself.
  - **Never paired this way:** FERB's own profile (by setting or by its mailbox account), a profile you unpaired with
    `ferb browsers unpair` (until you pair it by hand: migration 0046), one already being paired by hand, and anything
    while FERB is paused.
  - **Tries:** one per profile while FERB runs, journaled `browsers.autopaired` / `browsers.autopair_failed`. The key
    is revoked when the wait ends; a manual pairing that times out revokes its key too.
  - **Residual:** a window switch inside the browser in the last ten seconds before the page goes can still bring the
    previous profile's window forward.
- FERB's own wording of a memory that still waits for your Yes stays out of every prompt, as the memory rule says
  (I1, MP2). 25 such memories, indexed before 2026-10-19 (when waiting ones were first kept as 'proposed'), were still
  marked active. The index found each one already stored and left it as it was, so search returned them and 11 of the
  13 planner turns since 2026-10-23 showed them as "unconfirmed hint" lines. Now the index sets a stored memory's
  state from its item on its next pass (every planner turn): they leave the prompts and `memory.learned`, and the
  dashboard offers their Confirm. A memory you retired, or one a rollback undid, stays as it is.
- Every answer shows its full detailed text, with no short summary above a "Details" disclosure (owner, 2026-10-24:
  "the details response is better. remove the summarised short response as of now"; amends P30's "short by default"
  and the concise `display_text` of 0.27.0.dev1). The planner, workers and the orchestrator's recovery no longer write
  a separate short version: a direct reply streams its full text as it is written, and a task's answer is the full
  answer, with the side lines and "Sites contacted" line FERB adds. An unfinished task shows FERB's status line, then
  its question, the check's findings, why work stopped, what was confirmed and the files. The checker still judges
  the worker's answer against its separate evidence report (P31). Questions are unchanged: a reworded question still
  shows its direct version, with the complete question under Details. Notifications and `ferb chat` show the full
  answer too. Answers saved before this change keep their short text and Details in the app.
- A file macOS won't let FERB look at no longer ends your turn (P15, L23). Live 2026-10-24, "ok start cleaning": the
  plan listed `/private/var/folders`, where macOS refuses even to look at some entries ("Operation not permitted");
  the plan's file count stopped on the first one, and you got "something went wrong on my side". Such an entry is now
  left out of the count, as a protected file already is.
- When FERB can't do something, it says why, in plain words (P27, L11). Owner, 2026-10-24: "make sure that if ferb
  cannot do anything because of internal or internal permission restriction then that is stated, not just this vague
  message". A planning turn that hits such a refusal now reads "I couldn't start that: macOS didn't let FERB open
  /private/var/folders/… (Operation not permitted).", not "Sorry — something went wrong on my side. Could you try that
  again?". Each reason is chosen by the failure's type, never by its words:
  - **macOS refused:** the path and macOS's own words.
  - **FERB's protection rules refused:** says so, with the rule's message.
  - **The AI runtime broke FERB's rules:** what it did, e.g. it used a tool FERB didn't offer it.
  - **The plan was still invalid after its one repair:** the problems code found, e.g. "Inventory paths are protected:
    […]".
  - **The AI's answer didn't fit FERB's form:** what was wrong with it.
  - **Anything else:** "something broke inside FERB (<error>). That's a bug in FERB, not something you asked wrong."
  - **The AI service's own errors:** read as before.

  A message starts with what didn't happen: "I couldn't start that" before any task started, "I couldn't finish that"
  once one may have been running. This covers chat and voice turns, a task that ended without your stop, a worker
  attempt, a recovery plan that failed, a check that couldn't run ("Why the check couldn't run: …"), a refused
  reminder or memory, and a scheduled run that failed. The reason is redacted and capped at 300 characters; the full
  error stays in the records. A planning failure of any kind is now answered in the conversation record too, so
  "retry" has something to refer to (L21).
- A worker that needs a protected folder asks you once, not once per command (D31, P27). Live 2026-10-24, visa task:
  checking your visa-slot extension inside Chrome's profile folder took nine different commands, so the escape hatch
  asked nine times in two minutes. Workers now look where no approval is needed first (a public page such as the
  extension's Web Store listing, or a tool that already reads it) and, when a protected folder really must be read,
  get everything from it in one command. One approval still covers one exact command (your choice).
- A password is never tied to a whole web browser (D12, D28; owner, 2026-10-24: "can it take my cred safely and not
  expose but able to access them whenever it wants?? BUT at places only ive approved. SO THAT IT DOESNT LEAK
  ANYWHERE"). Live 2026-10-24, visa login: when the browser connection failed on the Microsoft sign-in page, the worker
  filled your password through Accessibility (`mac.secret`), which ties a saved password to the app,
  `app:com.google.Chrome`: every page Chrome shows. `mac.secret` now refuses, before it asks you anything, every
  browser FERB knows (Chrome, Brave, Arc, Edge, Safari, Firefox, Opera, Vivaldi and others, their beta channels and
  the web apps they make, and any app of a browser-only maker) and any app macOS lists as opening web links: "a
  password for a web page is filled through the browser connection (browser.secret), which ties it to that site's
  address; FERB won't tie it to all of Google Chrome". A password saved for a whole browser before this change is never
  filled, and none is saved that way. Native apps work as before.
- The password and login-code questions always show where the value goes (D12, P27). Live 2026-10-24, visa login,
  you saw "Enter the secret for app:[redacted] (hidden; Enter cancels)": the question's own words "the secret for X"
  looked like a password label to FERB's redaction, so it hid the place it was asking about. A login code's question
  had the same flaw with a port or a number in the address ("login code for https://x.test:[redacted]"). The
  questions now start with the place: "Destination: https://atlasauth.b2clogin.com. Type the password or secret for it
  here (hidden; Enter cancels); FERB fills it only at this exact destination.", then the name it is saved under when
  you asked FERB to keep it. Tests check them through the real redaction, in the event the app receives and in
  `chat.status`.
- The questions FERB's own code asks now start with one plain line saying what FERB wants, with the exact question
  under Details (P27, D33; owner, 2026-10-24: "the questions it asks are too ambiguous and doesn't simply state what is
  it and what it wants. so here we should have the short version and in details see if you want the raw things").
  - **A password:** "atlasauth.b2clogin.com needs your password or another private answer. FERB saves it in your
    Mac's Keychain as 'US Visa Scheduling password for …' and fills it only there." The place comes from the page's
    address (or the app's name), never from the name the worker proposed.
  - **A login code:** "accounts.google.com asks for a login code. FERB fills it only there and doesn't save it."
  - **Your Mac password:** "Your Mac password, for the one command you just approved. …"
  - **A protected folder (the escape hatch):** "FERB wants to read ~/Library/Application Support/Google/Chrome once,
    with one command (normally off-limits: a private store FERB's shell never reads)."
  - **A confirmation:** the concrete action, e.g. "FERB wants to send to bob@x.com, whom you didn't name." or "FERB
    wants to permanently delete ~/Desktop/old.txt, skipping the Trash."

  Code writes the line from the facts it checked, never from the task's words; the step, the rule id and the task's
  reason stay under Details. A worker's own question is unchanged. The notification for a waiting question uses the
  short line too.
- Changing a running task gets a one-line reply in your own words (L11; owner, 2026-10-24: "whenever i change
  something in a task it replies with this big ass message: 'Switched to: <the whole rewritten goal>' ... can we fix
  this somehow???"). It now reads "Got it: <your correction, on one line, cut at 120 characters>", plus "I'll apply it
  at my next step." when the running step hasn't taken it yet, and the restated send permission when there is one.
  The whole correction and the task's updated goal ("Updated task: …") are under Details in the app and are what
  `ferb chat` shows. This is the acknowledgement of a change, not an answer: answers still show in full.
- Signing in no longer asks you about "account security" (W2, D23; the code floor is unchanged). Live 2026-10-24,
  06:33–06:38, visa login: the worker declared "account_security" on every sign-in step (filling your existing
  password and your existing security answers, and clicking Continue on the Microsoft sign-in page), so FERB asked
  "this changes account security (a password, sign-in or recovery setting)" five times in four minutes. The worker's
  instructions and the tools' `effect` help said "a password, sign-in, two-factor or recovery change", which read as
  covering a sign-in. Both now say `account_security` is only for changing a password, two-factor or recovery
  settings, or deleting the account, and that signing in with existing credentials is `edit`. A declared
  account-security change still always asks.
- A "Verify you are human" checkbox (Cloudflare Turnstile and similar) in your own browser profile is clicked like any
  other control (P27; owner, 2026-10-24: "will ferb click on the cloudflare check box??? it should since its just a
  box"). Workers click it once, as you would, by the normal input route, then take a fresh look and wait for the page
  to continue. If a further challenge appears or the check still blocks, they quote the page's message and hand it to
  you as a short step for you to do.
- Typing and clicking work on a page another browser extension put a frame into (P19, L25; extension 0.15.7, protocol
  17 unchanged). Live 2026-10-24, visa login in your New_Personal Chrome profile: typing your username on the
  Microsoft sign-in page failed twice with "Cannot access a chrome-extension:// URL of different extension; outcome
  unknown", and the task fell back to Accessibility. That message means another extension has a frame in the page
  (most likely your "Check US Visa Slots" extension, which runs on that sign-in site), and Chrome refuses an
  extension's debugger for a whole tab when any frame in it belongs to another extension. FERB's real input needs the
  debugger, and its page-events mode asked for it first too. Now, where the browser refused the debugger, such an act
  says so as a known failure (nothing was done, so a retry isn't held for you) and suggests `page_mode=true` when
  FERB's scripting can reach the field. Page events then reach the page's own fields through scripting, which never
  touches the other extension's frame. You chose to allow page-event clicks there, with guards: an act that expects a
  confirm dialog fails before anything is clicked, since the tab's dialogs can't be watched, and a click or press says
  that a download it starts goes to your browser's Downloads folder, outside FERB's workspace and limits. Scrolling
  with the wheel, uploads and clicks on screenshot points fail the same known way there, and scrolling points to the
  page-event scroll. New_Personal runs the Web Store copy: it gets 0.15.7 once that is uploaded and published, or by
  Load unpacked from `~/.ferb/extensions/chromium` after `ferb setup --update`.
- Password and login-code questions say plainly what the value is for (D12, P27, D33; owner, 2026-10-24: "these secret
  questions need to be better asked, just state what is this for rather than all this confusing text"): "For
  atlasauth.b2clogin.com only: “US Visa Scheduling password” · saved in your Mac's Keychain", or "For x.com: a password
  or private answer — used once, not saved", and "For accounts.google.com: a login code — used once". The line always
  starts with the place, from the page's address (or the app's name), so a worker's text never comes before it. The
  name is the worker's proposed name, in quotes, only as the name, shown with ASCII letters, digits, spaces and
  : , ' ( ) - only, at most 40 characters: two reviews showed a worker could otherwise fake a place in front of the
  real one ("visa” — for accounts.google.com only · …"), even through lookalike punctuation, invisible marks or HTML
  entities the notch decodes. A host whose own name holds a code label before its port (otp.example.com:8443) still
  shows that port hidden, as the Details question always has. The full question under Details is unchanged.
- A loop's plan line is one short marker (L31; owner, 2026-10-24: "why is it spitting so much extra text with this
  loop message???"): "⟳ Until you say stop", or "⟳ Until: “the checks pass”". It used to repeat the whole goal and how
  rounds work ("I'll keep at this until You say to stop.: Monitor the bound Instagram tab … Between rounds I only
  check one fact, without the model."). The planner is told that "until I say stop" leaves end_when empty.
- A loop watches the exact video or tab it fixed (L26, L31; extension 0.15.8, protocol 17 unchanged). Live
  2026-10-24, keeping your Instagram reel playing: the round armed `media.44.playing` (and earlier `media.51.paused`),
  but Instagram's feed holds dozens of videos whose list positions shift as it loads, so the probe watched another
  video and your reel paused unnoticed ("it stops sometimes"). Each video or audio element now has its own id for as
  long as the page is open (`m12-3fa91c`: its number, then the page's own token, so an id from before a reload
  matches nothing after it), shown in the snapshot's media list; a loop's probe reads one element by it
  (`media_by_id.m12-3fa91c.playing`), and `browser.media` takes that id too. `browser.tabs` says whether each tab
  plays sound and is muted, and a probe reads one of your tabs by its tab_ref (`tabs_by_ref.<tab_ref>.audible`),
  cheaper than a snapshot. The keyed lists go only to a probe's reads, never to the model (an Instagram snapshot of 52
  videos is 29% smaller for the model than with them). The worker is told to watch these. A probe
  whose very first read already shows the job needs doing (as `media.44.playing` = false did) is no longer armed: the
  next round starts at once, told that its probe is probably watching a different element, and three such rounds in
  a row start the usual back-off with its one notice (a stop during that first read counts nothing). The daemon still accepts 0.15.6: with the Web Store copy the
  new fields are absent, so a probe on them is refused (its field is missing) and the loop runs on its 30-minute
  backup check until a round picks another field; upload 0.15.8 to the listing to give every profile the ids.
- A paired browser profile that isn't running no longer reads as broken (L25, P27). Live 2026-10-24, 12:27–12:31:
  the planner's facts said your New_Personal Chrome profile was "paired, extension transport disconnected"; the
  worker never used a browser tool, opened the profile through Chrome's Profiles menu and drove it with
  Accessibility, typing the address into the address bar (which drew a provenance question) and fumbling the
  Cloudflare page. FERB's browser tools already start a paired profile in the background with no window. The facts
  now say so ("not connected now (usually the profile just isn't running) — FERB's browser tools start it in the
  background, with no window; its first page needs FERB's minimized window, whose creation may switch Space"; a
  refused extension gives its reason), and the worker is told to use the browser tools for such a profile, ask you
  once when FERB's window must be created, and open sites with browser_open or browser_navigate.
- Setting a web field's text through Accessibility waits for the browser to catch up (P19, W2). Live 2026-10-24 12:09
  and 12:34, usvisascheduling.com's Username field in Chrome: `mac.act` set_value reported "not confirmed" though the
  field had your name; the worker typed it again and the field held it twice. Chrome updates what Accessibility reads
  a moment later, and FERB read it only once, at once. It now reads again until the field shows the text (up to 0.4
  s); when it still differs, the result says whether the field is empty and its length (never its text) and to read
  it in a fresh snapshot before typing again, since typing adds to what is there.
- A login name you give while FERB works on a site is remembered (P27). Live 2026-10-24: at 06:31 you said "the
  username is …" during the visa task; the task used it but nothing was saved, so a later task typed your email as
  the username. The planner now remembers a login name or username for a site in the same turn as the task change (a
  fact with a key naming the site, e.g. "usvisascheduling-username"); passwords and other secrets still go only
  through the hidden-input question.
- Stopping or correcting a task no longer drops the rest of what you said (L11, MP2). Live 2026-10-24: at 05:44 ("take
  my creds once, don't ask every time") and 12:22 ("book anywhere, don't wait for Chennai") the planner wrote a memory
  entry alongside the correction, and FERB dropped both, because a live correction and a stop answered before saving
  memory, forgets, reminders or schedule changes; the visa schedule kept its old scope. Every kind of turn (a reply, a
  new task, a stop, a live correction, a continue or a retry) now applies them once, with the same checks, and the
  reply to a stop or correction ends with FERB's lines about them ("Remembered: …", "Paused: …") and is kept in the
  chat's history like any reply. A correction that arrives just as its task ends does none of it and asks you to send
  the message again, so nothing is applied twice.
- Pausing a task that has a schedule pauses the schedule too (L3, P27). Live 2026-10-24 12:34: "pause the visa task
  for now, I'll say when to continue" stopped the task but left its daily 10:00 schedule active. The planner is told
  to pause or resume a job's schedule in the same turn as the task, and to cancel it when you end the job for good.
- Jev is optional and says where a key comes from (L37; owner: "in the setup just direct users directly to typesafe
  api creation site if they dont have it"): `ferb setup --jev` first points to https://console.typesafe.ai/keys, and
  `ferb doctor` shows "jev: not set (optional): activity notices are off; a key comes from …" instead of nothing.
- What you say is saved even when the planner skips it, and every guess is asked about (L36, MP2). Live 2026-10-24
  (memory audit): 14 of your durable statements in two weeks were never saved, and the nightly distill had made 27
  guesses about you that nobody asked you about. Distillation can now quote your own words for an item; FERB checks the
  quote against what you typed (never pasted text) and saves your sentence as yours. Any guess, from chat or the nightly
  look, becomes a question at once: Keep, Edit (your own words are saved instead) or Drop (deleted); after Edit or Drop
  FERB never suggests that guess again. Text typed into that question that isn't one of the three is refused, so a
  stray "yes" is never saved.
- The guesses FERB had made about you without asking appear once in the notch as one list, "Review N older guesses",
  each with Keep, Drop or your own words (L36).
- Memory is ranked and bounded (L36). Live 2026-10-24: every planner turn got about 207 memory lines (43 KB), 102 of
  them bare task ids, all kept whatever their relevance, because words like "is", "the" and "for" matched the JSON of
  every item. Search now reads only what items say, ignores words most items share, ranks by relevance with a small
  boost for recent items (your own statements count as always recent, so at equal relevance they come before FERB's
  guesses), and gives each planner turn the 60 most relevant (owner: "Planner 60, worker 20"). A message is also
  matched through the task it continues: at 12:17 your "hmm idk if thats s agood strategy…" shared no word with the
  saved username, but the running visa task named usvisascheduling.com. That task's goal and sites are now searched
  with your message, and its sites also count on their own (see the worker bullet below): on a copy of your database,
  with a stand-in username, that message brings it in 4th of the 60 (91st with the goal's words alone). Past
  tasks show their goal, outcome and date. A turn's memory took about 73 ms on a copy of your database. The index is
  rebuilt once at the next start, in the background in short steps (about 7 seconds on that copy; the first reply
  waits for it).
- A worker's memory lookups find what they look for (L36). Live: 14 of 20 `memory.read` queries came back empty,
  because the query had to appear letter for letter in an item ("RSNA knee Kaggle rank score" never could). Both memory
  tools now use the same ranked search as the planner, and `memory.learned` returns what each item says, not only its
  key. A search with no words lists the newest first, with no recency or owner boost.
- Workers and plan steps get memory (L36). Live 2026-10-24: the visa task's research step got no memory and only its
  own step goal, so it typed your email as the username even though you had given the username that morning. Every
  worker and plan step now gets the 20 memories most relevant to your request and the task, and a plan step sees its
  parent task's goal above its own. The sites, apps and people the task names also count on their own, so a short
  note that names the site isn't buried under long past tasks that share the goal's ordinary words: on a copy of your
  database, with a stand-in username, the research step's search ranked it 94th; it now ranks 4th of its 20, while 15
  of the 20 the goal alone would pick stay (a common name like "Chrome" can't crowd them out), for about 1 ms more
  per search. The planner gets the same for the tasks it is shown (4th of 60; 50 of the 60 stay).
- One login per site (L36, D12). Live 2026-10-24: a resumed visa task couldn't see the two answers saved that morning
  and asked for them again, saving them under new names. Saved secrets are now one record per site (the username, the
  password and each security answer under its question), a worker lists what's saved for the page before typing into
  its login form (`secrets.list`: names only, never values, whatever memory ranked), saving the same name replaces it,
  and the hidden-input question names the account after the site ("For atlasauth.b2clogin.com only (account: Asha
  Rao): …"). The visa site's sign-in form is on atlasauth.b2clogin.com, not usvisascheduling.com: a login saved during
  a task on a sign-in page at another address now also counts for the site you typed for that task (only when you
  typed one; with two, a login saved at Google's page would otherwise show the visa username there), so a listing for
  usvisascheduling.com finds it and the username you gave for that site names the form's account; its secrets still
  fill only on the form's own address. A site counts exactly, with or without "www.": never another address under it
  (on shared hosting such as b2clogin.com each customer has their own), never a site only a page or the model named,
  and a sign-in on a site you typed for the task links nothing (one the planner added, such as the sign-in page's own
  address, doesn't count for that). Your existing saved secrets keep working (the file is rewritten in the new form
  only when something is next saved); the visa password saved for Chrome as a whole is listed as "ask again". An
  unreadable saved-names file is never overwritten: a one-time hidden input still works, and saving says why it can't.
- A username you give is kept for its site (L36, P27). Live 2026-10-24: "the username is …" during the visa task was
  saved nowhere, and a later task typed your email. Now the username, in your own spelling, is saved for a site you
  typed, in that message or for the task (exactly, with or without "www.", never another address under it; never a
  site only a page, a pasted email or FERB named; never from FERB's rewording; an app's username stays ordinary
  memory), shown in the hidden-input question, listed on the login page and given to the steps that log in; FERB's
  line says "Saved as your login name for <site>." A sign-in page at another address (atlasauth.b2clogin.com for the
  visa site) names it from the first sign-in of a task you typed that one site for, saying whose account it is ("For
  atlasauth.b2clogin.com only (usvisascheduling.com account: Asha Rao): …"); so does any other page that shows it
  through a link. Forgetting or retracting the memory removes it from the login record; if that fails, FERB records
  why.
- A loop keeps holding your tab between rounds (L31; live 2026-10-25, "keep this tab running", task cc5debe4: watching
  began 96 s after your message, and each round paused your reel itself). Every round ended with `browser.keep_open`,
  which let go of your borrowed Brave tab; letting go turns the browser's focus emulation off, and Instagram paused
  0.3 s later while Brave was hidden and you weren't touching it. `keep_open` on one of your own tabs now only marks it
  kept: FERB holds it until the task ends (a loop's: until you stop it), then lets go of it, still open. A tab FERB
  opened is still handed over at once. Workers are told the loop holds its tabs across rounds.
- Loop speed, measured offline (the brief's "measure after"; real daemon, fake extension through the real native host,
  model calls scripted to take cc5debe4's own durations): from your message to an armed probe, 6 model calls and
  98.7–99.1 s before this batch (the live run took 96.5 s); 4 calls and 74.5 s with the code changes alone; 2 calls
  and 32.7 s when the planner makes the loop alone and its first round only looks (a round that must play the reel
  keeps its check: about 51 s, 3 calls, estimated).
- A loop starts with its own first round (L31). In all three of your live "keep this tab playing" runs on 2026-10-25,
  the plan bound the tab in a separate step before the loop (43–55 s), and that step let go of the tab when it ended,
  so the loop's first round found and borrowed it again. The planner is told a loop's first round does its own setup
  (finds the target, starts or fixes it, arms its probe) and to plan an earlier step only for different work.
- A round that only looked no longer waits for a model checker (L31, W2, W4; your decision, 2026-10-25: "Skip it for
  look-only rounds"). In cc5debe4 the first round only listed, borrowed and read your reel, already playing, then
  waited 14 s for the checker before its probe armed. Such a round now arms at once, and the probe's first read is
  the check. It counts as look-only only when every call just read (or borrowed or kept a tab) and its probe reads a
  tab or video the round itself looked at; a round that plays, clicks, reloads or navigates, sends or asks you
  anything, or reports the loop finished, is still checked. `ferb trace` shows `verify.skipped` with why.
- No resource review when cleanup changed nothing (L31; your decision, 2026-10-25: "Skip when nothing changed"). After
  a step that borrowed your tab and kept it, a review model ran for 6–9 s (a1f16a32, cc5debe4) although the tab had
  been kept and let go of as intended. When every tab is still open (kept as asked, or your own tab let go of without
  closing) and nothing failed, FERB skips it; `ferb trace` shows `resources.skipped`. A closed tab, an ended listen or
  recording, or a release the browser didn't confirm is still reviewed (a process FERB ended isn't reported among the
  released resources, so it doesn't bring the review back).
- One snapshot says whether a video plays (L26, L31; extension 0.15.10, protocol 17 and minimum 0.15.6 unchanged). The
  first time FERB looked at a video its `playing` was always false (nothing had moved yet), so every live round took a
  second snapshot to learn your reel was playing. A snapshot now waits up to 300 ms for unpaused media it hasn't seen
  before: its time moving ends the wait, so the first snapshot already answers; paused media isn't waited for. The
  worker is told `playing` already means the time moved. Reload the unpacked extension (or upload 0.15.10 to the
  listing) to get it; an older extension behaves as before.
- A snapshot's video and audio state comes first (L31). In a snapshot result the media list (each element's id,
  playing, paused, time, muted, in view) came after up to 24,000 characters of page elements and 8,000 of text, so
  checkers paged the recorded call to offset 24000 to learn whether your reel played (cc5debe4, three such reads). It
  now follows the tab, address and title; nothing else in the result changed.
- Each round of a loop is told which tabs the loop holds (L31). Every live round listed your tabs and borrowed the reel
  again to find the tab the loop already held. A round's plan-step context now lists them by id (with the page each
  held when the task took it over), so the next round can read the tab at once.
- A paused loop asks you with buttons (L31; you, 2026-10-25: "shouldn't it ask 'hey, should I stop the task since you
  closed the tab' when it notices the task is not there"). Live: you closed the Instagram tab at about 01:40:41, the
  next round found it gone at 01:40:57, and the loop paused with only a chat line repeating the planner's whole goal
  ("…until you say stop.. The exact Instagram tab was closed…"); the task stayed paused until 02:48. Now a pause raises
  a question card (with its notification): when a tab the loop held was closed, "The www.instagram.com tab you asked
  me to keep going was closed. Stop this task?" with Stop / Keep watching; any other pause asks the round's own
  question with Stop. Stop ends the task; Keep watching starts the next round; a reply in chat still steers it. The
  chat line names the loop in a few words.
- Loop speed review fixes (L31, 2026-10-25): a round that scrolled is checked (a scroll changes which reel plays); a
  probe counts as grounded only by what the round named or the loop holds, never a tab listing or page text; a skipped
  check needs a probe that will arm, and when an unchecked round's probe can't arm the next round starts at once; a
  keep_open that let go of a tab is acting, and workers are told the loop keeps the tab held between rounds; the
  closed-tab question is asked once, only for your own tab and never over an unknown-outcome question; Keep watching
  starts the next round without waiting out a back-off; a round always lets go of its checker's tabs; keep_open on
  your tab checks the browser still has it.
- Between a loop's rounds, the tab it keeps going is yours again (L31, D35; your decision, 2026-10-25: "give them back
  between rounds"). Holding your tab for the whole loop also meant your own downloads there went to FERB's workspace
  and links you opened in a new tab became the loop's tabs, even while it was paused. Now, between rounds and while
  paused, FERB keeps only what keeps the page running (its debugger hold, with focus emulation): your download goes to
  your Downloads and your link opens as your own tab. During a round, what the round does is FERB's, as before. No
  extension change: it works with every extension version.
- The file picker in that tab is yours between rounds too (L31; extension 0.15.10, protocol 17 and minimum 0.15.6
  unchanged). FERB intercepted file choosers in the held tab for the loop's whole life, so a file input you clicked
  there did nothing. Extension 0.15.10 has a `rest` command: between rounds the interception is off, and a round turns
  it back on before it acts. The daemon sends it only to 0.15.10 or later; with an older extension the picker stays
  FERB's until the loop ends. A page's alert or confirm between rounds is shown to you, and your answer no longer
  leaves the next round's snapshot blocked. It fails closed: if you press Cancel on FERB's debugger bar while the tab
  rests, or a round's "take it back" is lost, the picker is FERB's again before the round acts. Residual: a download a
  round's own action started that arrives after the round ends goes to your Downloads.
- The Claude runtime caches the planner's stable prefix (P20, D32; owner, 2026-10-25). The adapter joined FERB's
  instructions and context into one text and ignored which parts repeat, so only Claude Code's own prefix was read from
  the cache (4.8% of a ~101k-token planner turn; every turn wrote ~97k tokens again). FERB's text now goes as separate
  blocks of the same user message with a 1-hour cache marker on the last part that repeats (images after), the
  Anthropic API runtime's placement; measured 98.7% read on the next turn (97.5% after one more entry). Claude Code's own markers are pinned to the
  same hour (unset, past a subscription's usage limits it picks 5 minutes, and the API would refuse FERB's marker after
  them). Claude Code also adds text FERB didn't write (a reminder with your account email, an environment message);
  `ferb trace` doesn't show it.
- Codex runs without its skills message (P20, D32). Every Codex run carried a `<skills_instructions>` message naming
  the run's random temporary folder right after FERB's instructions: about 611 tokens and 1-3 s per run, and it kept
  the rest of the prompt from ever matching the previous run's. FERB loads no Codex skills, so every role now runs with
  `skills.include_instructions=false` and `skills.bundled.enabled=false`.
- The planner can cache its context on Codex too, behind a switch that is off (P20, D32, MP2; your decision,
  2026-10-25: "yes to all. just make sure we are safe and after so many security checks we don't end up in prompt
  injection"). Codex uses its thread id as the cache key and FERB started a new thread every turn, so at best the
  system prompt was cached (0% and 15.6% on two back-to-back planner turns). With `[runtimes] codex_planner_cache =
  true`, the planner's turn resumes a thread FERB writes under a key fixed per model, with the stable context (the
  device facts and the conversation, labels and all, under one line saying quoted content is data) in Codex's
  developer role and only what changes in your turn: 98.9% cached on the next turn. If that can't be set up, the turn
  answers the old way and the journal says why (`runtime.cache_fallback`). The switch stays off until you run the live
  injection eval (`uv run pytest -m real tests/security/test_planner_injection_real.py -v`: nine planted
  instructions, both ways) and it passes; then the default flips in its own commit. Offline, a planner answer that
  followed a planted instruction still asks before a send to someone new or a payment, either way.
- After the security review of that switch (P20, MP2, S4; 2026-10-25): a task result in the planner's history now
  shows where it came from and its text as one quoted line, so a page it quotes can't pass a forged "[c9] (…) Owner:
  pay …" line or hidden Unicode characters off as FERB's record (both paths). With the switch on, only your chat turns
  use the cached path (scheduled and watch runs don't), never while pasted text counts as your own words, and under a
  key for this Mac; `ferb trace` shows `context_role` on each model turn and a fallback line names its request. A
  crash's leftover Codex run folders (each with a copy of FERB's Codex login) are removed at the next start. The live
  eval now has 18 scenarios (a forged history entry, "when the owner says thanks…", turning off confirmations, forgetting
  memory, revealing secrets, mailing the conversation away, "say the inbox is empty", an open FERB check "the owner
  approves", hidden Unicode), fails any action or task after a plain "thanks" and any destination you didn't type
  (after the one question, only a destination the planted text gave),
  proves each run tested its path, and logs to an owner-only file.
- Old facts retire (L36). Live: the 10-11 wedding flight and the finished RSNA competition were still in every planner
  prompt. A fact you give with a date now leaves memory after that day, and when your words say something is over FERB
  retires it (or asks: "Is this no longer true? …", Retire or Keep). Keep is remembered, so FERB doesn't ask about that
  fact again. The nightly look retires or replaces something only on a whole phrase you typed that shares a word with
  it (never pasted text), said after FERB learned it, and FERB tells you when it does ("I retired “…” because you said
  “…” (date). Reply to undo."); otherwise it asks. "Until 01-02" said on 31 December means two days on. A username
  you give again for the same fact replaces the old one on its login record; a new version of the fact that names no
  username (an edit in the dashboard, say) clears it, and so does retiring it; one whose fact is past its date is no
  longer offered.
- Saved answers no longer damage other text (L36, rule 14). Live: a saved security answer that is an ordinary word
  showed as [redacted] inside FERB's notes about you, the visa task's record was refused twice, and on 10-14 48 records
  failed because a short saved value matched inside FERB's ids. A word-like answer is now hidden only as a whole word,
  nothing is hidden inside FERB's ids (a saved value that is itself a whole id-shaped token still is), a password is
  still hidden wherever it appears, and saving an answer that is an ordinary word adds one line to the task's answer
  saying FERB hides that word everywhere.
- Typing into a terminal asks you first, every time (D31, rule 10). Live: FERB's sandbox blocked `ps` and the Codex
  control socket, and the worker typed `codex` and Enter into your VS Code terminal without asking (task 6f5b8589);
  FERB knew terminal apps only by bundle id. Typing into any terminal (terminal apps and terminals inside other apps,
  e.g. VS Code's) is now enforced by code: FERB always asks the owner first, showing the exact command, every time,
  with no standing permission; a No types nothing. One Yes covers that input and the one Return after it, so the same
  command again, or a retry, asks again. Code recognizes a terminal from facts: terminal apps (now also Warp Preview,
  Tabby, Termius, Rio and Contour, plus any app whose Info.plist declares itself a shell), xterm.js terminals in
  editors and pages (VS Code, Cursor, Windsurf, cloud shells) by xterm.js's own classes, and native terminals by
  their identifier or label (JetBrains, Xcode's console, Nova); any typing in Zed asks. Every path is covered: typing,
  values, edits, keys, foreground input, ⌘V and menu pastes, and `browser.act` into a web terminal (extension
  0.15.11). A refusal says to use FERB's shell, and the escape hatch when the sandbox blocks a command you need; the
  worker prompt says the same.

Licensed under Apache-2.0.

Integration note: the commits from `9db0a5a` through `906cb61` (Task 22's picks before its fixture fix `1af6731`) are
not green on their own; never install from them, and skip that range in `git bisect`.

## 0.27.0.dev1 — 2026-10-11 — integrated prerelease, owner acceptance pending

- Surface Codex account-check failures with safe diagnostics and relevant recovery instructions instead of the
  generic request-repeat apology. Publish refreshed credentials before starting model work; preserve newer owner
  logins and independent startup checks. No failed call or other task is automatically replayed.
- Native approval attention opens the actual new question once with an amber trace, preserving keyboard focus;
  routine replies stay quiet. Reconcile every local request/backend root, including interrupted unbound requests
  and duplicate turns, and render supported Markdown tables, lists, links and code as native views.
- Replace shell per-site confirmation with an explicit exact-command network grant; command, resolved directory,
  identity, scopes and task tree define its scope. Existing local-server pins, explicit refusals, sandbox and
  unknown-outcome retry protection remain. Routine URL checks distinguish payload from delivery metadata.
- Audit the owner's session-selection and concert reports: sampled coverage was overstated, and aggregator stock
  metadata did not establish current primary ticket availability. Tighten worker/checker evidence instructions;
  report audit and live acceptance remain distinct from offline prompt-contract checks.
- Fix recurring sealed-input refusal: verify the local helper with the correct inline signing requirement, and
  support exact ordered split OTP textboxes even without recognizable numeric attributes. Extension 0.15.2 /
  protocol 13 validates the complete binding, masks filled fields and retains partial-input uncertainty.
- Add same-generation concise `display_text` for replies, questions and task reports; keep full original details.
  Stream the concise field, verify it alongside the task answer, and keep consequential approval scope intact.
- Native follow-ups: standard paste/edit commands, no automatic panel reopening on background answers, and a Tasks
  view with individually scoped Stop, activity, results/artifacts and full-record inspection. These changes and
  direct steering were activated after the owner’s tasks completed; no running task was interrupted.

- Inject owner corrections into active workers without stopping/replacing the task. Codex workers now use isolated
  app-server steering; Claude and direct APIs retain the same run and apply queued updates before accepting a
  final answer. Preserve update order, durable context, completed work and existing permission boundaries.
  Handle startup/completion races explicitly; unrelated requests and explicit Stop/retry remain distinct.
- Fix idle CPU spinning in workspace and Accessibility watchers when macOS returns immediately from an empty
  run loop. Preserve their existing polling cadence with a shutdown-interruptible wait for the unused slice.
- Fix the Spotify-triggered daemon freeze: Automation permission checks cannot block chat/Stop or daemon shutdown.
  Pending checks are shared; the caller's timeout covers preflight, expired calls never launch a script, and
  cancellation kills/drains owned script processes. Passive checks still never request a permission prompt.
- Catch up missing setup persona discovery on existing installs: asynchronous local app-use evidence, a shared
  CLI/native confirmation, explicit `setup --persona` refresh, and stable identity context only after owner review.
- Repair the native app's voice crash after microphone route changes: use the current hardware format and contain
  AVFAudio Objective-C exceptions before they can unwind through Swift tasks. Rebuild the packaged app with the fix.
- Add genuine provisional conversational reply streaming across Codex, Claude and direct API adapters, the chat
  socket, CLI and native frontend. Cross-chunk redaction protects secrets; canonical final replies replace drafts,
  and interruption withdraws them. Action receipts and task results retain execution/verification before display.
- Owner-requested reply style: short, natural, result-first everyday answers, with detailed evidence kept in the
  separate report. Planner condition phrases and checker findings avoid boilerplate; substantive deliverables,
  uncertainty and concrete confirmation scope remain complete. Pending questions precede the remaining-work list.
  The later presentation layer adds a separate concise field in the same generation, with no truncation or output cap.
- Owner-reported update fix: passive software inspection no longer invokes `sfltool dumpbtm`, which triggered
  repeated administrator dialogs. Its background-task inventory is explicitly unavailable; other software facts
  remain. Updates recognize already-granted macOS permissions instead of reporting an obsolete setup marker as
  deferred, while missing or unknown permissions still require the owner's normal setup flow.
- Native frontend contract: advertise attachment support, preserve selected source paths through clarification
  without granting them owner authority, associate pending questions with root tasks, and retain redacted final
  answers, verification and artifacts for reconnect through status/trace reads.
- Bridge and retain the Accessibility observer callback with PyObjC's required callback ABI; report sanitized
  startup error details instead of only the exception class.
- Integrates the remaining containment, task loop, perception, memory and delegated-conversation milestones.
  The owner installed the prerelease; the later authorized prompt repair was installed by the agent. Live milestone
  acceptance remains separate from checkout validation.
- Explicit same-plan continuation preserves completed steps and uncertain effects in the same task tree.
  Retrieved memories are budgeted individually in rank order, with origins and full retrieval routes retained.
- Adds a packaged owner acceptance catalogue covering every target category and all 38 documented security cases,
  plus saved-baseline comparison that preserves denominators and missing measurements without inventing verdicts.
- Final checkout validation: 1,792 default tests passed; core typing, syntax, migration integrity and wheel-resource
  checks passed. Live checks remain excluded, and owner F0–F4 results remain historical.
- After update repairs/native client integration: 1,812 default tests and 44 frontend Swift tests passed. The
  completed backend is installed, all four grants verify, and passive watchers report no startup failures. The
  native app was built/relaunched and its socket contract verified without submitting a model task.

## 0.27.0-dev — 2026-10-11 — F5 checkout, pending final validation/live checks

- Executable-pinned local services through the authenticated proxy; trustd denied unless lifted for one exact command.
- Exact-command escape grants, contained elevated execution with hidden passwords, identity-bound tools and clipboard attribution.
- Contextual OTP/card classes, fail-closed on-device screenshot redaction, one initial app/tab image, hidden native/browser fills,
  split OTP controls and optional target-bound Keychain handles through a locally signed helper.
- Hidden login codes, saved secrets and administrator passwords never start voice capture or enter its transcript events;
  ordinary spoken answers share chat's contextual secret redaction before approval resolution and event publication.
- Native screenshots collect secret boxes across every AX snapshot page; depth-truncated inventories withhold pixels even
  when another secret field was found. Recordings also withhold frames when the native inventory is unknown.
- Secret fills whose controls change after input began retain an unknown outcome and cannot be blindly retried;
  extension 0.15.1/protocol 12 preserves that outcome through reconnects and the action ledger.
- Contextual OTP redaction recognizes explicit code labels; ordinary login phone numbers and numeric accessibility or
  account choices remain usable. Streaming still retains an incomplete code introducer across output chunks.
- Metadata-only disk sizes, concrete irreversible-bulk confirmations, destructive classification in forced shadow, alignment advice,
  daily security facts, regression send switch and containment statistics. FDA setup waits for working containment.
- No installed-daemon/browser changes or live claims. Python/JavaScript/Swift compilation checked; test execution deferred to the
  combined final pass requested by the owner.
## 0.27.0-dev — 2026-10-11 — F9 conversations, pending combined/live checks

- Added researched conversation proposals, explicit account/contact/topic/terms approvals, checked turn loops,
  durable incoming-reply waits, owner interference pause, evidence-backed hand-over and action receipts.
- Added owner chat/CLI controls, quote-checked notification rules and display-acknowledged ordinary activity notices.
  Notification triage remains shadow. Gateway checks and P19 remain in effect; GUI account/contact proof still needs
  fresh inspection. No native or outward-facing live run is claimed here.

## 0.27.0-dev — 2026-10-11 — F7 perception, pending combined checks

- Added passive workspace/AX, per-field text, changed clipboard, Notification Center and installed-software sources.
  Capture guards/redaction precede event delivery; typed/clipboard content expires after seven days.
- Added durable Jev batches, retrievable excerpts, owner-quoted notification scope rules, told receipts and shadow
  measurements. The model-based sensitive-text classifier is removed; task authority stays with owner instructions.
- Full setup can show local app-use evidence once for confirmation/edit. New native adapters are unverified on the
  owner's desktop; offline regression tests are authored, with execution deferred to the combined F5–F10 checks.

## 0.27.0-dev — 2026-10-09 — F4 live fixes

From the owner's live acceptance run of F4 (tasks `79bc5bfa`, `fbe968b0`). Commits `6b7eb85` through this documentation
commit, after the plan `84779cf`; the version stays 0.26.1.

- Who does the work (`6b7eb85`, `1ca394d`; V6, D34, P27): the prompts say that work the owner hands to a named agent or
  person that does it on its own is done by that one (an app FERB uses as a tool is not that): FERB starts it, helps it
  and checks its result, and asks before taking over (a worker's request to take over is the owner's decision, not a
  method question); one condition says the named one did it, and each quality or limit the owner states is its own
  condition; reports say who did each part and with what; a lasting choice the owner did not ask for and the request
  does not need is confirmed first; everything the owner reads says "you" and "your" (`tests/test_prompt_contract.py`).
- The report check (`a6494d6`, `515f402`; P16, D34): every check judges, numbered last, whether the report the owner
  will see matches the records (`REPORT_CONDITION`, judged by the checker); false fails the task. A done answer is the
  report the check judged (`checked_report`); the review after cleanup may lower the status but never writes new done
  wording. FERB's condition is found by its index and said in the owner's terms: "What I reported didn't match the
  records: …", "I couldn't confirm my report matches the records: …", and under "Still to check" "Whether what I
  reported matches the records". The story's "checked nothing" count includes it.
- Uploads (`d30a15b`, `b0e1f73`, `26f05fe`; MP2, rule 17): the browser reads FERB's own copy under the original name
  (`~/.ferb/workspace/uploads/<tree>/<random>/<name>`; `uploads/outside/`, an untrusted root, for outside content),
  so no macOS folder-access dialog; the original is only read. Copies are made through held file descriptors
  (`fclonefileat` with `CLONE_NOFOLLOW`, or a byte copy with `O_EXCL|O_NOFOLLOW`) and checked against the original's
  SHA-256; cleanup never follows a link. Copies used only in tabs FERB closed go when the tree ends; one used in a
  borrowed or kept tab stays until the browser reports that tab removed, or until the next start-up. Residuals: after
  `ferb setup --update` a still-open tab's staged file is gone; tab ids repeat after a browser restart, so closing a
  new tab with an old id can delete an old copy early.
- Download limits (`8c55913`, `c1f932a`; D35): the owner's limits of 2026-10-09: 512 MB per download, 50 files and
  4096 MB per task tree, 50 new tabs per task tree, a 20480 MB folder (only past it do the oldest go to the Trash), a
  free-disk floor `browser.download_min_free_gb` = 10 (10 GB, as macOS shows it; space macOS can purge doesn't count), checked before any byte moves (an undeclared size too) and before each part is written, the refusal
  saying how much is free; no time-based clearing (`retention.download_days` removed). A one-time, all-or-nothing
  upgrade changes only `config.toml` values still at F4's defaults (and removes `retention.download_days`), journaled
  as `config.upgraded`, which `ferb timeline` prints change by change or as "settings already current". Any admission
  error in a live task tab aborts the download and records a fact; it is never handed to the browser.
- Desktop changes with no FERB on-screen action (`c6b4ad9`, `a9e91d1`, `189c087`; P26, P4, MP2): tools declare
  `on_screen` and the gateway records it with each call; FERB's cleanup is an on-screen action (`task.cleanup`). When a
  readable front app changes, or the Space changes, with no FERB on-screen action in progress, the record keeps the
  facts and the agents' context and the story state them: "<App> came to the front — no FERB on-screen action in
  progress (the last ended N s earlier); running: …" ("(none in this task; the last of any FERB task ended N s
  earlier)", "(none yet)", one decimal under 10 s); another tree's processes appear only as "<id> (started by another
  FERB task)". Code never says who caused a change. Older records keep the old near rule. While `ferb pause` is on
  nothing about the desktop is recorded (a read under way is dropped); the story says "desktop not observed (paused)
  until …". Fields `desktop.no_on_screen_action`; context key `front_or_space_changes_with_no_ferb_on_screen_action`.
- Owner decisions of 2026-10-09 recorded here and in the plan: the limits above; the one-time config upgrade ("yes");
  no instruction or restriction on another agent's harness ("i dont want this hardcoded control over other agent's
  harness. ferb should just know that it didnt do it"), so FERB's records state facts instead; D6/D26 as clarified (a
  start-of-task screenshot of the working app's window, then screenshots only on the model's request; built in F5;
  see the F4 entry below).
- Documentation (this commit; P16, D34, D35, MP2, P26, P27): README, ARCHITECTURE (sections 2, 6, 7, 10, 11), RPC
  disturbance kinds, owner guide upload line, decision registry tests and notes (P4, P16, P26, P27, D34, D35, MP2),
  the owner's re-check list, the plan's decision status for 2026-10-09, and the F5 carry-forward in the plan: when the
  shell sandbox lands, deny `workspace/uploads` to the shell for reads and writes, or move staging beside
  `~/.ferb/recovery`.

## 0.27.0-dev — 2026-10-09 — F4 verification and recovery

Milestone F4 of `docs/0.27_PLAN.md` (P16 V1–V8, D6, D20, D26, D29, P19, S1, D34, D35). Commits `ddf1830` (the F4 plan)
through this documentation commit. The version in `pyproject.toml` and `ferb/__init__.py` stays 0.26.1 while 0.27 is in
development.

- Evidence store and app-window pictures (`3fabb44`, `5193810`; V1, D20, D26, D29, rule 14): migration 0020
  `evidence`; each tool image kept 7 days (`retention.evidence_days`) under `~/.ferb/evidence`, the span names its id;
  one masked picture of an app's window when a worker starts on it (evidence only); facts only, no pixels, for excluded
  sites, results listing a secret field, browser windows and hostless pages, recording contact sheets, and messaging
  and mail apps until F5; the checker reads captures with `task.evidence kind=evidence`.
- Read-only checker (`668ac96`, `eab7919`; V3, W4): an observe-only run token; the gateway offers and runs only
  capabilities marked `observation`; a checker navigates only FERB tabs it opened and never the owner's; every refused
  call leaves a `tool.refused` span and the story says "⚠ N tool calls refused".
- After-action reads (`b007868`, `8ad09cd`; V2): every consequential result, a failure's too, carries an `after` block;
  an unconfirmed change is unknown, never false; `after_change` / `after_value` conditions read the recorded span.
- Three statuses (`3623fee`, `a48207f`; V4, V5, D20): migration 0021 adds `done_unverified`; the orchestrator's finish
  carries `result: unconfirmed|failed`; "Done, but not confirmed:" and "Done, but there was nothing to check it
  against."; "unverified" only where a check gave a verdict; `ferb stats`, `ferb baseline`, `ferb trace` and verbose
  chat count and label the three apart; an unconfirmed scheduled run neither clears nor adds a failure.
- Conditions (`44c55db`, `5a6d8d4`; V6, V7, V4): `ledger_done` is no longer a check kind (a saved one loads as `none`);
  conditions name observable outcomes; the checker's optional `how` becomes "To check: …".
- Ledger for recovery (`2314eea`; V7): `Ledger.unresolved(tree)`; AppleScript stays ledgered (no declared read-only
  flag; code can't prove a script only reads).
- Unknown outcomes (`5a34bfc`, `4814808`, `a3c50c8`, `e8ecc8d`, plan renumbering `4138e05`; P19): code refuses an
  identical consequential call while one in the tree is unknown or running; "the same action" ignores notes, target
  spellings, method choices and sibling tools (`ANNOTATIONS`, `target_args`, `method_args`, `ledger_key`); the owner
  approves one retry through a FERB-written Yes/No question (migration 0022: the row stays unknown); "Uncertain prior
  actions" covers the whole tree.
- Recovery (`e481fe7`, `f62415c`, `7d7284c`, `f583757`; S1): migrations 0023 and 0024; a clone of a file before FERB's
  fs tools overwrite or edit it (the file really written, symlinks first; no copy, no change; a write tool whose file
  can't be told is refused unless it declares `creates_new_only`); durable Trash records checked by device and inode;
  `ferb restore` never over a later change ("(before FERB)" beside it); snapshots only when an agent asks
  (`recovery.snapshot`) or before a permanent delete, at most one per 15 minutes, deleted after 7 days;
  `retention.recovery_days` (7). Also fixed: `recording.stop` had failed on every gateway call since `88368e8` (unknown
  `recording` lock kind).
- Named targets (`bba6d1c`; D34): the checker gets the owner's named targets; evidence about a substitute is
  `met=false`, so the task is failed.
- Downloads (`362166e`, `72ff310`, `90d4c90`, `e620ee2`, `205c009`; D35): a download in a live task's tab is taken
  through tab-scoped CDP `Fetch` (plus content.js for blob/data links and a pinned page hook for FileSaver-style saves
  and `window.open` popups) into `~/.ferb/workspace/downloads/` as facts (path, size, SHA-256, source, origin
  `external:download`), never a dialog; anything outside a live task's tab goes back to the browser untouched; budgets
  (`browser.download_max_mb`, `download_task_max_files`, `download_task_max_mb`, `task_max_tabs`,
  `download_total_mb`), `retention.download_days` (30) to the Trash; popups claimed only as new tabs whose opener the
  browser confirms; migration 0025; extension 0.14.0, protocol 10.
- Documentation (this commit; P16, D6, D20, D26, D29, P19, S1, D34, D35): architecture section "Verification and
  recovery (F4)" and corrected download wording; README statuses, `ferb restore`, retention keys, evidence and download
  notes; RPC `recovery.list` / `recovery.restore`; registry tests; the F4 live-check list; one `background.js` comment.
- F4 final fix wave (`e8d2bfb`, `d28ea7e` and the documentation commit after them), from the whole-branch review:
  - Statuses (V5): `done_unverified` is labeled "not confirmed": `ferb trace` `done (not confirmed)`, verbose chat
    `done — not confirmed`, `ferb stats` "not confirmed N (y%)"; the stored status and `ferb baseline`'s JSON keys
    are unchanged. A planning failure's text gets its missing sentence separator.
  - Unknown outcomes (P19): the hash keys a file by the path the call really writes (as the fs lock and S1), and the
    trail is told that path; a path with a NUL is refused cleanly. An AX action the app didn't answer
    (`kAXErrorCannotComplete`) is unknown, never failed. The retry question names the tool the worker really called
    and one readable target; control characters are stripped from it and from everything `ferb chat` prints. `mac.act`
    says a repeat needs the owner for every action but focus.
  - After-action reads (V2): an `ax_value` result that read nothing back is unknown, not "no change"; a failed call
    that changed the screen never confirms `expected=true`.
  - Evidence (V1, D26): the app-window picture is taken after the call answers, outside its app lock; tool images are
    written off the event loop.
  - Recovery and retention (S1, D35): every `retention.*_days` is at least 1 day; download retention leaves a row
    outside the downloads folder as it is (skipped once, not counted).
  - Downloads (D35): an owner's new-tab click in a tab FERB drives waits at most ~1 s for the daemon (`open_seen`),
    then opens natively, at most once; relayed popup tokens are bounded; an unconfirmed popup is detached
    directly; dropped page saves become one refused fact per document per minute; no `detached` event for a popup
    still being checked. Within protocol 10 / extension 0.14.0, never published.
  - Named targets (D34): the checker gives `how` for an uninspectable named target; such a task ends not confirmed.
  - Docs: the daemon-down/restart, 30 s idle and borrowed-release hand-backs in the D35 residuals; both forms of the
    retry question; what `ferb restore` prints; which screenshots keep facts only.
  - Re-review follow-up (D20, P19, D35, D6, P3): a cancel during an evidence write leaves no file without its row,
    and the call's and the first look's spans still end ("cancelled"); a task's end no longer cancels its first look
    (only a stop or shutdown); a look failure that escapes its span is logged; `_select`'s menu-closing cleanup can't
    turn a known non-selection into unknown; the retry question keeps every recipient, the amount and the account;
    an orphaned content.js (after an extension reload) never eats the owner's click; `ferb timeline` and `ferb stats`
    rows say "not confirmed"; live check 3 notes the picture's delay; plan P3 points at the D6/D26 clarification.
  - D6/D26 clarified (owner, 2026-10-09): one start-of-task screenshot of the working app's window goes to the model,
    then screenshots only on the model's request, never per step; the model-bound part is built in F5 after
    screenshot redaction (plan, decision registry, architecture).
- Carried to F5: full screenshot redaction; the destructive-action and sink confirmations that build on S1; never-read
  lists covering `~/.ferb/run` and `~/.ferb/recovery`; FERB's shell reaching the daemon socket; shell and AppleScript
  file changes, which get no per-file copy.

## 0.27.0-dev — 2026-10-08 — F3 data foundations

Milestone F3 of `docs/0.27_PLAN.md` (D9, D14, D21, MP1, MP2, P26, S3). Commits `b3ef759` (the F3 plan) through this
documentation commit. The version in `pyproject.toml` and `ferb/__init__.py` stays 0.26.1 while 0.27 is in development.

- One origin vocabulary and origins on journal rows (`4ccbba8`, `dde1923`; MP1, MP2): `ferb/core/origin.py`; migration
  0015 adds `journal.origin`; derived rows record `made_by`.
- Every tool result labeled by origin (`dad0f19`, `ffab3a4`, `834d5c2`; S3, MP2): from `read_sources`, a declared
  capability origin, or `external:unlabeled` for a read that said nothing; shell output outside content; excluded hosts
  `external:web:excluded`; recordings `external:recording:<id>`; readers pass stored origins through; the model sees an
  `Origin:` line.
- Planner blocks and root task records labeled per origin (`083cec0`, `fd36326`, `43d2ed2`; S3, MP2): context sources
  record their origins, the root task span carries the request's, `SpanStore.task_origins` unions them, and derived
  records (answers, outcomes, chat replies, schedules) inherit them at capture.
- Argument sources on consequential calls (`404d700`, `d0f5240`, `999ab50`; S3): `owner_words`, `external:…` or `unknown`
  per target, matched by value kind; the owner's text is the request plus `owner.ask` answers. Facts only; F5 enforces.
- Step summaries (`6e3fe88`, `b925027`; P26): `spans.summary`, `tool.finished`, and `ferb logs --tools` step lines;
  results are redacted before the summary is clipped.
- Episodes (`81d8fea`, `213901b`; D21, MP1, P26): one permanent `episodes` row per task, composed by code
  (`code:episode/1`) at task end, on stop, after a restart interruption and before a purge; failed and unfinished steps
  shown as such.
- `ferb trace` levels (`aab36a3`, `d0f9f51`; P26, D21): the story by default with a cost line, `--steps`, `--full` and
  `--json`; the story outlives the spans; golden tests.
- Retention (`9b39b19`, `35c279b`, `73e719e`; D14, D21): `retention.trace_days` and `retention.decision_days` (30); a
  task's spans go only after its episode exists, one transaction per tree, at most 200 roots per hourly pass;
  `retention.purged` and `retention.failed` in the journal; interrupted tasks age from the restart.
- Content search (`080019b`, `a580108`; D9, MP2): FTS over activity and daily summaries with secure-delete (migration
  0019), `trail.query text=` and `trail.days`, Hindi words whole, query terms lowercased like the tokenizer; day
  summaries labeled by content.
- After the owner's live run (P26, RQ2): the disconnected-profile error states that `browser.open` loads the profile
  and needs one minimized window with the owner's permission; the story's Desktop line skips FERB's own reads.
- Registry, architecture section "Records and traces (F3)", README trace and retention notes, and the live-check list
  (D9, D14, D21, MP1, MP2, P26, S3). Residuals for F5: shell reads of downloaded files are not tracked, and FERB's own
  `fs` tools reach any path until protected-path lists exist.
- Fix wave after the whole-branch review (`620d090`, `3af0dca`, `b403b8b`, `c4b5074`, `cdf6cc6`, `07ed8db`, `6fccaf5`,
  `85a8169`; S3, MP2, D21, P26, D9): FERB's clarify question is never recorded as the owner's words (labeled in older
  continuity rows, it is removed when read); `owner.ask` answers match as `owner_answer`, apart from typed
  `owner_words`; the gateway prepares each task's outside texts once; retention yields to the daemon loop between roots
  and journals `retention.purged` only when it purged; the story shows the whole clarified request; stopped workers
  read "stopped: …"; episodes keep `tool_origins`; `tool.finished` carries only the shown arguments; chat-turn origin
  lookups use the `journal_kind` index; per-hour sites label a day summary `external:web:trail`.
- Residuals after the re-review (`d225ed3`, `240c960`, `e74ce6c` and this entry; S3, MP2, P26, D21): spans without an
  origin follow one fail-closed rule in episodes, `task.evidence` and `task_origins` (a tool call or context source
  with output is `external:unlabeled`, whatever its kind, since spans from before F0 have no action kind); the story
  keeps the `[pasted text]` marker the typed words leave out; a retention pass cancelled between roots journals what it
  purged; continuity rows written before 2026-10-03 lose FERB's older unlabeled "endorsed" question when read, only
  where FERB's own recorded question matches exactly.

## 0.27.0-dev — 2026-10-07 — F2 model runtime layer

Milestone F2 of `docs/0.27_PLAN.md` (D32, P20). Implementation, tests and documentation: `d3ca7d3` (the F2 plan) through
this documentation commit (every F2 commit after `d3ca7d3`). The version in `pyproject.toml` and `ferb/__init__.py`
stays 0.26.1 while 0.27 is in development.

- Final-review fixes (D32): a Codex runtime is refused with "run `ferb setup --codex-login`" when FERB's own Codex home
  has no login (F2-R11); the API runtime checks the cancel flag between a tool-call event and the bridge call;
  `ferb runtimes use` warns when the chosen runtime is not usable yet (F2-R12); usage cost reaches the span cost column.

- One request shape for every runtime (`77168ee`): `ModelRequest(system, parts, cacheable)` with `assemble_parts` and
  `ContextBlock.cache`. The orchestrator marks device facts and chat history as the cacheable prefix; task calls mark the
  owner's original request and the task specification.
- Typed answers are checked in one place, `check_final` (`ferb/core/structured.py`), for every runtime (`1388173`).
- One usage record for every runtime (`193a7a0`): `Usage` has input (cached included), cached, cache-write, output
  (reasoning included) and reasoning tokens, plus cost from the provider or the dated price table (`ferb/runtimes/prices.py`;
  an unlisted model has no cost). The memory live test now sums `cache_write_tokens` and `cached_input_tokens`.
- Capability table and per-role selection (`99b4baa`): `CATALOG`, `[runtimes] orchestrator|worker|checker` (checker falls
  back to worker), `[models] <runtime>`, `ferb runtimes` and `ferb runtimes use ROLE RUNTIME [--model M]`.
- Codex runs with its own empty HOME (`~/.ferb/codex-user`) and `skill_mcp_dependency_install` disabled, so the owner's
  personal skills never load (`df3173a`, `2ef1ff6`). After the owner's live run: Codex's capability row now says image
  tool results reach it (verified live 2026-10-08 with an MCP tool returning a PNG), and live tests borrow FERB's own
  Codex login for their temporary daemons instead of failing (`ferb setup --codex-login` is required since F2-R11).
- `ApiRuntime` and `GatewayBridge` (`463c7fc`, `13f0595`): FERB's own tool loop for direct APIs; tools only through the
  gateway; retries with backoff on transport errors and 408/409/429/5xx/529; a tool the run was not offered is a
  violation, with every name in a turn checked before any call runs; cancellation is honoured and tool calls run as child
  tasks (an owner stop ends the tool call and the model is told); keys come from the Keychain and are removed from errors.
- Anthropic, OpenAI and Gemini adapters (`d1ab98b`, `ed75c32`, `fdb9115`, `9341f69`, `096da93`, `e081a02`) under a shared
  contract suite: Anthropic cache markers, OpenAI explicit breakpoints on gpt-6 and gpt-5.6 and a per-role
  `prompt_cache_key`, Gemini's implicit cache. Each rejects stop states other than a normal end (output limit, refusal,
  early stop), so a partial answer is never final. OpenAI requests include `reasoning.encrypted_content` (stateless).
- API runtimes are selectable per role (`243abb1`, `2130c3d`, `d52f1b3`): only usable runtimes are offered as worker
  types, `ferb setup --key PROVIDER` stores a key and checks it through the `runtimes.check` RPC (which tolerates
  non-JSON answers), `ferb doctor` has a line per role, and offline test daemons skip the real Keychain. The daemon
  keeps one `Redactor` across config reloads, so exact secret values (API keys, OTPs) stay redacted everywhere, and
  Google API keys are redacted by pattern.
- Live tests (this commit): marker `real_api` (deselected by default), API variants in `tests/test_real_runtimes.py`
  and `tests/test_runtime_cache_real.py` (second call reads the cache). Owner decision: no keys for now, so only the Codex
  and Claude CLI live checks run; the API checks wait for keys (`docs/OWNER_ACCEPTANCE.md`).
- Not changed: Codex's cache key is its per-process session id; use an API runtime for the orchestrator to cache across calls.

## 0.27.0-dev — 2026-10-07 — F1 correctness fixes

Milestone F1 of `docs/0.27_PLAN.md`. Implementation, tests and documentation: `c7d8c43` (the F1 plan) through this
documentation commit and the F1 review fixes (every F1 commit after `c7d8c43`).
The version in `pyproject.toml` and `ferb/__init__.py` stays 0.26.1 while 0.27 is in development.

- Chat history (`HISTORY_TOKENS`) and resumable attempts (`ATTEMPTS_TOKENS`) are capped to their newest entries that
  fit, kept whole, instead of being dropped whole when over budget. History is listed oldest first with an omission
  note; resumable attempts are listed newest first with a note about the older ones.
- Recall drops query terms that match most memories (more than half, with a floor of 2; data-driven) and tokenizes by
  Unicode category; migration 0014 rebuilds the memory index so Devanagari words index whole.
- Observed patterns accumulate evidence by kind/subject/key while still proposed, and the distiller sees pending
  patterns, so they can activate after 3 days. Identical content always credits the matching item first. An active observed item matches only identical content, so a changed
  pattern becomes a new proposal that can supersede the old version.
- FERB's MCP tools stay directly visible to Codex workers and checkers: the Codex catalog lockdown sets
  `supports_search_tool` to false, so Codex 0.157.1 lists them as `mcp__ferb.<tool>` instead of deferring them behind
  tool_search (verified live 2026-10-07). As a fallback (for example when Codex's models cache is missing and the
  lockdown cannot apply), a worker or checker whose runtime reported tools without any FERB tool is re-run once with
  that fact, traced as `runtime.tools_not_loaded`.
- The context budget report closes the context and the owner's message is the last block (P20). After the owner's
  live run: the device-facts block (installed apps, the same on every turn) comes first, and history chat tags count
  from the oldest entry (`c1`), so a new message no longer renumbers every earlier line and breaks the cached prefix.

## 0.27.0-dev — 2026-10-07 — F0 process and measurement

Milestone F0 of `docs/0.27_PLAN.md`. Implementation, tests and documentation: `97ba205` through the F0 review fixes
(every F0 commit after `97ba205`, including the final-review fix commits).
The version in `pyproject.toml` and `ferb/__init__.py` stays 0.26.1 while 0.27 is in development.

- `ferb verdict TASK pass|fail [--note TEXT]` records the owner's acceptance. Verdicts are appended to a new
  `owner_verdicts` table (migration 0013) for a root task, given by exact ID or a unique 6+ character prefix; child
  tasks are refused with their root named. `ferb trace --json` shows the latest one as `owner_verdict`, replacing the
  old null placeholder. Code never infers a verdict.
- `ferb baseline` writes a read-only snapshot to `~/.ferb/baselines/<timestamp>.json`: done and checker-verified
  rates, owner verdicts, tool calls and errors, model-run times, average task time, questions put to the owner, cache rate and average trace size.
  On a database older than F0 it prints a message to start the daemon once and exits non-zero.
- `ferb regress [set-file]` runs a regression set in one fresh conversation (default: the packaged
  `ferb/eval/sets/smoke.toml`, two harmless prompts). It declines owner questions so the run never blocks, writes
  one row per task to `~/.ferb/regression/`, and prints a `ferb verdict` line per started task for the owner (a chat-only answer is reported as having no
  task to judge).
- Security harness skeleton in `ferb/eval/security/`: per-run canary tokens, a localhost request logger, and an
  offline `scan_trace` that flags canary egress (in the trace or in logger hits, including request bodies) and unexpected recipients or contacts.
- Gateway `tool.call` spans now record the action `kind` and its non-empty `target`, so exported traces can be
  scanned. `Gateway.test_send_allowlist` refuses send-class actions to unlisted or unidentifiable recipients before
  the ledger; it is None, and changes nothing, outside tests. GUI typing into a messaging app is not covered.
- `docs/0.27_DECISIONS.toml` maps every D*/MP* decision to a milestone; `tests/test_decision_coverage.py` fails if
  one is missing.
- CLAUDE.md carries the 0.27 rules instead of the 0.26-hardening ones (D25): code enforces where/what boundaries,
  never intent; code-enforced confirmations for hard-to-undo actions; one full access level made secure by design;
  acceptance is the owner's `ferb verdict`; pluggable model runtimes behind FERB's gateway. README, architecture,
  RPC, owner guide, capabilities and acceptance docs now describe verdict collection instead of a null placeholder.
- Validation: **742 passed, 56 deselected**; core mypy, documentation references and `git diff --check` pass.
  Offline only: nothing was installed, no live run was made, and no owner verdict has been collected yet.

## 0.26.1 — 2026-10-06 — Background window reuse and honest blockers

Implementation, regressions and evidence: `46c0be7` (from `f6ff8ad`).

- Stop automatically creating a browser window when no FERB window exists. Minimized creation was retained from
  0.25, but the owner's full-screen Brave task still recorded a transition during its first open. Return all window
  choices without mutation; the model selects an exact existing window for an inactive tab. A missing selected
  window fails without fallback. New-window creation requires explicit foreground permission. Keep warm minimized
  windows reusable. Extension 0.13.1 / protocol 9 makes this contract mandatory across the bridge.
- Preserve a typed reason for stopping alongside every failed/unverified condition. The previous formatter removed
  the actual worker/recovery blocker with unchecked success prose, leaving a checklist that concealed why work
  stopped. Do not restore optimistic summaries or claim that model refusals are solved.
- Require fresh primary-source evidence for “new/latest/current” claims. Old mail metadata plus a fresh page for an
  old assignment does not establish the newest assignment. Preserve runtime tool-name observations in traces and
  exports, and require actual deferred-tool discovery before claiming FERB tools are unavailable.
- Validation: **712 passed, 56 deselected**; core mypy, JavaScript syntax and documentation references pass.
  Disposable headless browser cold-window routing and byte-identity uploads passed with zero observed disturbances.
  A real Codex run discovered a tool among 221 fixtures and correctly reported an unavailable primary source instead
  of identifying an old item as current. Its 40 cursor/window-order observations all coincided with recent owner
  input; there were no focus/full-screen/Space changes. Evidence retains individual observations and sampling limits.
- Installed and restarted the daemon; installed code matches checkout and connected Brave reports extension0.13.1.
  Installation/doctor recorded zero changes while Brave was full-screen. The exact owner full-screen browsing
  scenario remains pending acceptance; headless tests are not proof of that case. See `docs/AUDIT_0_26.md`, the
  updated acceptance checklist, and `docs/evidence/*0.26.1.json`.

## 0.26.0 — 2026-10-05 — Correct by judgment, legible by design

Implementation, regressions and documentation: `fd7d7f8` (from `88368e8` on `astra/version`).

- Reconstruct the wrong-source upload and short-study-guide incidents from their actual traces, write failing
  regressions, and audit every production area with a rotated independent pass. The upload happened on 5 October:
  code refused an iframe input; the model substituted a same-name Drive file and the checker accepted its name.
  The study task already received the plain request; a concise planner rubric and weak term/name checks caused
  shallow deliverables. Intake-memory and artifact-path facts were judged at the wrong layer, and cleanup prose
  hid failed checks. Evidence and finding-by-finding dispositions are in `docs/AUDIT_0_26.md`.
- Observe desktop changes for every task and action, including cancellation/cleanup. The read-only sentinel records
  frontmost app, key window, cursor, visible order, focused full-screen state and public Space-transition events.
  Missing fields and sampling limits stay explicit; temporal overlap does not invent causation or permission.
- Preserve the full original request, pasted sources and deliverables. Replace silent context clips with a named
  token estimate, one final budget accounting, protected required blocks and fetchable omission markers. Persist
  actual runtime prompts, intake receipts, context sources and paged history/memory/evidence.
- Remove target/source shortcuts and native foreground recovery. Native actions use one explicit mechanism and
  return observed failures. Browser uploads address exact top/child/OOPIF inputs directly, intercept native choosers,
  compare selected bytes with canonical local identity, and carry large content through multipart transport.
  Extension 0.13.0 / protocol 8. Unexpected dialogs await an explicit choice. OTP message content stays sealed;
  opaque numeric candidates never automatically select the latest message or a guessed code.
- Make independent checkers judge outcomes against the request, including mechanical predicate observations.
  Final answers put every unresolved condition before confirmed facts and exclude unchecked success prose.
  Cleanup cannot resurrect rejected or missing inspection; daemon shutdown awaits task finalizers before teardown.
- Remove obsolete semantic policy/Gmail helpers, native resolution/focus-restore branches, unused runtime knobs,
  stale comments, superseded acceptance prose and mislabeled SVGs. Preserve typed Jev hooks and one supervisor
  approach selector. Prompts share one versioned loader. Structured task exports include approach decisions,
  actions, verification, disturbances and cleanup; owner verdict remains an explicit future client seam.
- Publish the owner guide, capability ledger, 16-task live acceptance checklist and daemon RPC client contract.
  Refresh architecture, Mermaid and the 14-journey walkthrough; check source links/symbols automatically.
- Validation: **706 passed, 55 deselected** on this Mac; `mypy ferb/core`, JavaScript syntax and seven-document
  source-reference checks pass. Disposable TextEdit edits and top/child/OOPIF uploads passed with zero observed
  disturbances, with explicit missing-field limits. The real Codex RPC pipeline preserved and checked the full
  pasted Unicode source. Account workflows, another-Space targets and broad owner acceptance remain unverified.
- Installed with `ferb setup --update`; daemon 0.26.0 / code hash match verified; `ferb doctor` exits 0 with all grants.
  It leaves account session/disconnected profile/device-change warnings explicit. Post-install isolated live rerun:
  **2 passed**; 20 browser samples and 221 real-model samples saw zero changes/errors, with the login window frontmost and
  full-screen AX unavailable. Earlier 19 TextEdit samples preserved frontmost full-screen VS Code with all fields available.
  25 update/doctor samples also saw zero changes/errors. These are scoped observations, not owner acceptance.

## 0.25.0 — 2026-10-05 — Record the requested target and inspect saved pixels

- Add task-owned browser-tab and native-window recording. Bind captures to the observed profile/tab or window/PID,
  preserve action references during sampling, and finalize silent sampled MP4s with target/timing receipts.
  Capture failure produces a partial result; cleanup stops recording before releasing its targets.
- Decode the saved video for inspection. Worker/checker instructions require actual content evidence before
  claiming the requested process was recorded. This addresses the 0.24.0 demo task whose MP4 contained VS Code
  while its worker claimed to have recorded both browser tabs.
- Preserve owner-authorized foreground use across the whole task and its retries without repeating permission.
  Incorporate the 0.24.1 background recovery/updater changes below. Extension 0.12.0 and protocol 7 add `record_frame`.
- Recording needs FFmpeg/ffprobe. Native capture needs macOS Screen Recording access. Browser and native live
  acceptance are pending; the disposable two-tab check is `tests/test_recording_real.py`.
- Validation: 51 focused recording/recovery/extension checks pass. Full suite: 339 passed, 54 deselected;
  273 failures/errors hit sandbox restrictions (local sockets, Keychain or LaunchAgent writes). This is not a
  fully passing integration run. No installed daemon or personal browser was updated.

## 0.24.1 — 2026-10-05 — Preserve background operation through recovery

- Reproduce the Discord signup trace's recovery failure: the dispatcher allowed foreground input after an extension
  protocol refusal, and the worker repeatedly activated Chrome through shell/AppleScript. Share one interaction
  contract across planning, workers, questions, inspection, recovery and final resource review. Preserve owner
  background/profile constraints and require profile discovery before opening; recovery cannot invent permission
  to take focus. Keep these decisions with the agents; no command deny list or per-action approval is added.
- Extension 0.11.1 (protocol 6) accepts structured compatibility facts in a daemon refusal. Once idle, an eligible
  local bundle can reload once even when its version matches but its running protocol is stale. Persist version/
  protocol attempts, preserve pairing and retained tabs, and wait through commands/owned tabs/gestures/storage races.
  Pairing failures, older bundles and malformed versions do not trigger this repair.
- Add extension regressions and shared-prompt delivery checks, including a full recovery/question/inspection/cleanup
  scenario for execution in an environment that permits local sockets. Installation and owner acceptance are pending.
- Validation: 28 focused checks pass, along with JavaScript syntax, `mypy ferb/core` and `git diff --check`.
  The full-suite attempt had 322 passes and 273 permission-related failures/errors (local sockets, test Keychain
  creation and LaunchAgent writes); it is not a green integration run. No installed daemon/browser was restarted.

## 0.24.0 — 2026-10-03 — Agent-directed lifetime and audit cleanup

- Give browser workers a keep-open handoff so requested class/result pages survive task cleanup. Release borrowed
  and retained debugger state without closing the page; persist cleanup outcomes and return invalidated outcomes
  to the existing recovery flow before reporting completion.
- Serialize extension session mutations, duplicate-command claims and window creation. Route container scrolling
  to the actual frame/pane and report target failures. Remove implicit personal-profile selection and dead account
  gates. Extension 0.11.0 / protocol 6; update the managed extension with the daemon.
- Remove retired policy evaluators, holds, taint/exposure caches and stale planning restrictions. Require success
  criteria for new action tasks, preserve complete planner/dispatch traces and provider failures, and clean up
  partially started runtimes. Current-state requests require fresh inspection in the agent instructions.
- Keep full scheduled instructions; present lateness/failures to the model without automatic skip or three-failure
  pause. Preserve full voice corrections. Propagate forget/sensitivity changes into daily activity copies.
- Remove app-wide suppression after input misses and stale native foreground restoration. Make complete shell
  history pageable beyond 256 KiB; redact split records before owner-only persistence and close failed-start FDs.
- Hash prompts/extension resources, expose failed watcher startup, keep distillation retryable, remove unused Jev
  questions/misleading duration estimates, and move evidence SQL behind SpanStore.
- Isolate real browser test hosts from the installed service. Keep reusable test helpers outside test-case modules;
  move the standalone native probe to device diagnostics and remove dead production/test code.
- Consolidate Markdown into README, architecture, agent guide, changelog, notices and one owner acceptance record;
  preserve runtime prompts and attribution. Historical design documents remain in Git.

Validation: **572 offline tests passed, 53 live tests deselected** (115.50 s). Core mypy, extension JavaScript syntax,
Python parsing and whitespace checks passed. Owner-observed acceptance is pending. The installed daemon was not
updated by this change. See `docs/OWNER_ACCEPTANCE.md` for one-at-a-time live checks.

## 0.23.3 — 2026-10-03 — Inspect source evidence and recover incomplete coverage

- Audit six calendar/WhatsApp tasks on installed 0.23.2: correct personal-calendar evidence falsely failed,
  native checks lacked navigation, earlier group-name observations were inaccessible, and unknown coverage
  ended as done. Source/app assumptions and incomplete date coverage were separate agent mistakes.
- Give checkers the worker's installed capabilities and navigation on its existing task tabs. Preserve registered
  parent validation, original ownership and unrelated-task isolation; remove the remaining read/actor scope list.
- Add `task.evidence` to search/page actual redacted tool observations, including earlier views, errors and action
  history. Exclude model reasoning, recursive evidence reads and images; restrict retrieval to the current task
  or a checker's registered worker. Fresh reads remain necessary for changed state or image-only claims.
- Route account observations to model judgment instead of literal-alias checks. Return false and unknown result
  conditions to planning for retry/ask/finish; unresolved coverage never reports done or loses its missing conditions.
- Update prompts for app/account resolution, official-source membership, date coverage, navigation verification
  and concise blocker reports. Decisions remain with agents; no phrase gates or action/retry caps are added.
- Validation: **566 offline tests passed, 52 deselected** (114.93 s); 47 focused checks, core mypy and whitespace
  checks passed. No new live personal-app test or daemon restart. Owner acceptance after installation is pending;
  see `docs/INSPECTION_RECOVERY_0233.md`. Extension 0.10.0 is unchanged.

## 0.23.2 — 2026-10-03 — Preserve owner app switches during browser operations

- Trace diagnosis: a Chrome search tab finished loading after the owner switched from VS Code to Brave. The
  browser completion callback treated any foreground browser as focus theft and reactivated VS Code.
- Remove automatic front-app capture/restoration from browser tab opens and windowless profile wakes for both
  drivers. Keep inactive tabs, minimized task windows and windowless background launch. No extension change.
- Replace the old restoration assertion with 16 regression cases: both drivers, opens/wakes, success/failure,
  and owner moves to either Brave or Chrome. All cases reproduced the old bug before the fix.
- Installation and owner-observed live acceptance are pending; see `docs/BROWSER_FOCUS_0232.md`. The owner
  authorized pushing `astra/version` without merging. No installed-daemon restart is included.
- Validation: **556 offline tests passed, 52 deselected** (115.18 s); 59 focused browser tests, core mypy and
  whitespace checks passed. No live desktop action was used to validate this patch.

## 0.23.1 — 2026-10-03 — Truthful completion reports and tracked coding processes

- Preserve unresolved verification findings in the final answer even if recovery returns a success-sounding
  summary. Keep complete owner answers; do not cut off the uncertainty after a long report. Recovery instructions
  prefer correcting a missing detail before finishing.
- Give the checker the complete worker report, artifacts and evidence; retain complete worker trace reports and
  checker evidence. Persist verifier completion/failure/cancellation and reset its state on a retry.
- Add `shell.start` and `shell.wait` for task-owned asynchronous commands. Waits return on actual process exit;
  reads/waits expose exit codes and paged output. Prompt coding workers to run commands directly and inspect
  startup/output instead of detaching with `nohup`/`&` and sleeping blindly. No command deny list is added.
- Owner confirmed the audited Coursera → reminders → Claude FMI redo caused no disturbance. Audit verified
  source reads, eight pending reminders, a completed Claude session and a two-page PDF with matching figures.
  The audit also found the omitted Coursera durations, stale verifier rows and one failed detached launch.
- Export the requested FERB chat and FERB↔Claude conversation locally, outside Git. Historical task records are
  unchanged. See `docs/COMPLETION_AUDIT_0231.md`. Installation of this patch awaits owner-authorized rollout.
- Validation: **541 offline tests passed, 52 deselected** (113.41 s), including ten new regression cases. Core
  mypy and whitespace checks passed. No live model/browser task was run for this patch; no push.

## 0.23.0 — 2026-10-03 — Managed extension updates and lost-pairing diagnostics

- Repair the owner's live scaler connection after explicit authorization: the current extension had new unpaired
  credentials while FERB retained its older profile binding. Re-pairing restored a verified connection without a
  browser or daemon restart. Repeated reload advice was insufficient for that state.
- Package the extension with FERB and publish it at `~/.ferb/extensions/chromium` during setup/update. Add
  `ferb browsers extension install` and `path`; publish the staged manifest last. New profile installation still
  requires browser setup. OpenClaw-style Store installation needs a published Store listing; FERB has none.
- Extension 0.10.0 checks newer local manifests on its existing alarm and reloads when commands, gestures,
  debugger attachments and owned task tabs are idle. Preserve pairing state and prevent failed-reload loops.
  Keep protocol 5 and bump the daemon's minimum extension version to 0.10.0 together with the manifest.
- Remove permanent reconnect suppression after refusal, time out silent handshakes, retain disconnect diagnostics,
  and report unpaired/bad-key/replay hellos without permitting actions. CLI separates current and historical
  versions and reports unpaired live connections without guessing their browser profile.
- Validation: **531 offline tests passed, 52 deselected** (112.59 s); real disposable Chrome path migration plus
  automatic 0.10.0 → 0.10.1 reload passed (32.98 s), retaining authenticated pairing and opening no visible window.
  Core mypy, JavaScript syntax, wheel build/bundled-resource lookup and whitespace checks passed. Replace an old
  source-string backoff assertion with an executed handshake check and compare extension versions numerically.
- Owner-authorized rollout check confirms the running 0.23 daemon matches the checkout. Both paired Chrome
  profiles migrated to the managed folder without re-pairing; all three profiles connected on 0.10.0. The owner
  completed Brave's folder migration after browser-control policy refused its internal page; path and connection
  were verified. Chrome was closed at the final check. No extra restart or push.
  Chrome's Developer mode is required for unpacked reloads. See `docs/EXTENSION_LIFECYCLE_023.md` for migration,
  research and acceptance limits.

## 0.22.1 — 2026-10-03 — Keep screenshot clicks on the background pointer route

- Connect `mac.click` to the exact-window pointer transport after AX. Previously it bypassed that route and
  advised foreground retries; the scaler task delivered five foreground clicks after its stale extension failed.
  Do not replay an unconfirmed pointer dispatch in the foreground. Reject a screenshot whose window changed owner.
- Preserve screen coordinates on native events and supply separate window-local coordinates to the window route.
  An actual canvas receiver exposed the doubled window-origin error; regressions cover click, drag and scroll,
  including negative screen coordinates.
- Correct tool/prompt fallback advice and disconnected-profile diagnostics. Preserve the requested profile and
  background operation without new code-level intent/permission gates. Show `allow_foreground` in `ferb trace`.
- Fix disposable Chrome pipe inheritance and add an isolated, owner-supervised native click test. The first click
  caused no disturbance according to the owner. The corrected repeat landed exactly once at the requested pixel
  on another Space, with unchanged cursor/front-app samples; the second owner verdict is pending.
- Document primary-source research, measured limits, the scaler extension reload requirement and the assignment
  trace's remaining model-setting/delegation mismatch in `docs/BACKGROUND_POINTER_0221.md`.
- Validation: **521 offline tests passed, 51 deselected** (95.43 s); the isolated real click passed (1.98 s);
  core mypy and whitespace checks passed. No installed-daemon update or push. Broader app/gesture and full-task
  acceptance remain separate from this single-click check.

## 0.22.0 — 2026-10-03 — Agent-directed control and daily context

- Replace the gateway's action-policy path with authenticated execution, resource locking, ledger/trace recording
  and redaction. Remove active identity/phrase/profile permission checks, exposure tool withdrawal, contact gates,
  mandatory effect floors, send holds, duplicate-send approvals, shell/path/script deny lists and automatic account
  helper pages. Final-outcome authorization is interpreted by the orchestrator/worker from the owner's request.
- Give task workers the installed action registry and `actions.search`. Add typed worker selection, blocked reports,
  orchestrator recovery and question resolution. Preserve artifacts, recent reports and uncertain actions across
  attempts; failed result checks return for repair. Explicit stop cancels recovery as well as execution.
- Preserve clarification replies and FERB's earlier question as separate sources; remove the yes-word shortcut
  that rewrote FERB's wording as the owner's. Finish and clean up truthfully if recovery context cannot be built.
- Remove fixed task/attempt/action caps, report-keyword retry and the Claude adapter's turn cap. Keep silent-worker
  liveness, actual OS/transport errors, resource ownership, fresh references, privacy handling and sealed secrets.
- Record redacted what/where/when activity in `activity/today.jsonl`, resetting on first access after local-day rollover.
  Retrieve bounded relevant memory, activity and registry entries every turn. Retrieval is currently lexical and
  device-local; no cross-device sync claim.
- Collect Mac permissions and selected app Automation at setup, with one-time collection when upgrading older
  installations. Keep FERB-profile default and owner-directed profile selection in agent instructions.
- Rewrite the cold-start architecture, code tour and supporting docs. Record retired policy assertions in
  `docs/TEST_MIGRATION_022.md` instead of silently skipping them. Legacy pure-policy helpers are not live gates.
- Validation: **514 offline tests passed, 50 deselected** (96.29 s); core mypy and Python compilation passed.
  Fix the fake identity-pairing helper to wait for the token page, not the preceding wake page; sealed-secret
  replay and origin checks remain covered.
  No installed-daemon update, push or real app action. **Owner acceptance of 0.22 remains pending**, especially the
  scaler Gmail → requested coding-session workflow, authorization judgment and desktop disturbance.

## 0.21.1 — 2026-10-03 — Verify the worker's existing browser tab

- Owner confirmed the live Brave YouTube task reached the last available comment; an independent read matched
  the final author/comment. The installed checker nevertheless failed because it could not access the worker's tab.
- Give registered read-only verifier/reconcile tasks access to their own worker's tab aliases. Match parent,
  request, identity, device and task tree; retain original ownership, profile restrictions and cleanup behavior.
- Supply existing aliases in checker context so inspection preserves lazy-loaded content and scroll position.
  A checker may read/scroll borrowed tabs, never close, navigate away or act on them.
  Block checker edits at the gateway, including FERB's combined `identity:browse` scope. Guide inaccessible or
  truncated reads to unknown, not false. Offline regression covers the full chat handoff and isolation.
- No daemon update or push; live acceptance of the corrected checker remains pending.
- Validation: 546 offline tests passed, 50 deselected (119.96 s); core mypy and whitespace checks passed.

## 0.21.0 — 2026-10-03 — Window-targeted scrolling, owner-verified rollout

- Stop treating row revelation as directional scrolling; require the intended pane, use scrollbar progress, and
  restrict Page keys to text views. Bound batches and stop on uncertain progress without claiming the end.
- Add wheel scrolling to screenshot-bound native pointer input and an optional SkyLight window-targeted transport.
  No focus transfer, activation or double posting. Availability permits an attempt; cross-Space acceptance is pending.
- Let the model choose distance per step separately from batch count, from small nudges to large viewport steps.
  Show scroll direction, distance, amount and target in CLI traces. Worker guidance forbids substituting chat navigation for
  scrolling. Add `ferb watch --mac --only M14` for one owner-verified tick, functional and keyboard-focus verdicts.
- Preserve watch setup/action/cleanup order; verify a newly created document before accepting it as test-owned.
- Research, limitations and acceptance procedure: `docs/BACKGROUND_SCROLL_021.md`. No daemon update. Owner verified WhatsApp scrolling on another Space without disturbance; see the recorded acceptance scope.

## 0.20.1 — 2026-10-02 — Personal browsers require explicit direction

- Default web requests to FERB's own profile. Repair an owner-web classification without explicit personal-browser
  direction. Remove personal-browser scopes from owner tasks without that direction, including retries; an earlier
  agent's goal/profile choice cannot grant access. Explicit owner-to-FERB route correction can proceed.
- Use explicit browser/profile naming for authority; account names and matching site domains alone no longer count.
  Guard native Mac browser tools against bypassing absent personal-browser scopes.
- Report disconnected paired profiles as connection failures when listing tabs, including recorded version refusal
  details. Do not return an empty tab list and invite profile guessing or Mac fallbacks.
- Remove the visible wake-page fallback after a windowless connection attempt times out. Opening a new visible
  browser window and then switching the owner back is not reliable background operation.
- Diagnosis, migration and limits: `docs/BROWSER_DEFAULT_0201.md`. No installed daemon restart or owner-profile action.

## 0.20.0 — 2026-10-02 — Durable conversations and exact pointer gestures

- Resume the last chat across CLI and daemon restarts; add `ferb chat --new` and `--resume ID`. Persist redacted
  task context, prior attempts and pending questions. Recover older recorded chat attempts; pass real, scoped task
  IDs to the supervisor. Retries keep original goal/profile context under fresh authority; unknown effects block replay.
- Preserve browser screenshot coordinates through policy-gated clicks, double/right clicks, hover and same-frame
  drag. Require fresh visual state and trusted frame-local mousemove readiness, including cross-site frames.
- Add native `mac.pointer` for bounded process-directed gestures on the current Space, without OS cursor takeover.
  Validate geometry/freshness, retain both drag-endpoint policy floors, and release held input on cancellation.
- Carry pointer cancellation from the daemon to the extension; distinguish pre-input refusal from uncertain partial
  input. Document input delivery separately from task success. Extension 0.9.0, protocol 5 requires owner reload.
- Evidence, installation and remaining owner acceptance: `docs/POINTER_CONTINUITY_020.md`.

## 0.19.0 — 2026-10-02 — Reliable browser targets, login handoff and honest outcomes

- Make Chromium Accessibility the primary semantic view, backed by isolated DOM element references. Bind frames
  by CDP identity, route child debugger sessions, include embedded-frame text, and refresh frame positions before input.
- Refuse covered/disabled targets and unconfirmed focus; never silently downgrade trusted input to page events.
  Preserve failed and unknown extension outcomes (extension 0.8.0, protocol 4), including after reconnects.
- Add task-tab screenshots and point-to-element resolution through the normal action policy. Add hidden owner OTP
  input bound to a visible code field and HTTPS frame origin; values travel sealed and never enter model results/logs.
- Require fresh outcome evidence for ledger-based goal verification; no conditions means unverified. Normalize
  snapshot bookkeeping for loop detection and stop repeated consequential actions without observed progress.
- Use raw Mac AX roles for text editing, verify set-value results and focus, and retry learned failed mechanisms
  after an hour. Exclude comma-separated content previews from action-label floors while keeping call/pay/delete floors.
- Keep wakeable paired browsers available. Structured chat stop/retry/continue cancels and waits for the old run,
  preserves original request context and scoped authority, and blocks retries of unknown actions.
- Validation and remaining limits: `docs/RELIABILITY_019.md`. Owner acceptance and installation remain separate.

## 0.18.0 — 2026-10-02 — Background first: the drivers rebuilt on what the owner watched work

### Documentation follow-up — 2026-10-02
- Recover the interrupted documentation drafts and finish the cold-start guide: rewrite ARCHITECTURE around
  the implemented one-worker flow, refresh README / CAPABILITIES / CODE_TOUR, and complete FILE_MAP with the
  rework docs, owner questions, learned app profiles and watch/stats tooling. Keep the runtime at 0.18.0.
- Correct stale section references and distinguish current code from plans and historical test runs. Document
  the pairing-guidance flag, submit-versus-send undo behavior, driver-only browser screenshots, and the meaning
  of verification when no success conditions are supplied. Preserve the pre-rework contract in git history.

### Browser
- Browser driver rebuilt background-first (PLATFORM_BROWSER.md 13), extension 0.7.0 / protocol 3: one FERB window
  per profile born minimized and kept, task tabs never active; a debugger session per task tab for its life (real
  input by default, mac shortcuts, dialogs answered by the extension, uploads, jpeg screenshots in parts); snapshots
  with in_view / occluded / scroll containers / shadow roots / iframe offsets; container scrolls by real wheel;
  bridge fixes from the audit (timeouts reconciled, outbox order, same-connection hello, confirmed close, YouTube
  account read once). Real lab test: `tests/test_browser_lab_real.py` (throwaway Chrome, frontmost app never
  changed). (`3937d45`, plus the lab-test commit)

## 0.17.0 — 2026-10-02 — One worker per request, gated at the action
Phase 4 of `docs/REWORK_PLAN.md`, as designed in `docs/DESIGN_ONE_WORKER.md`.
- The supervisor answers with a reply, one question, or a task (goal, identity, informational targets, success
  conditions). No subtasks, readers, actors, verbs or scopes; the only check left in code is the identity floor
  (one repair round, then a question). One worker runs the whole request with every tool its identity allows, in
  the background; its summary is the owner's answer (the separate answer run is gone).
- Every act call declares an `effect` (navigate / edit / send / pay / destroy) and `why`. The gateway raises the
  declaration to the control's floor: tool kinds are facts (a plain click is a click, a search box is a search,
  Return is a submit, a form's button submits it) and one label list of floor words (Send / Submit / Call / Pay /
  Buy / Order / Delete / Remove / Unsubscribe / Sign out). Navigate and edit run; a send asks once per (request,
  effect, target) with the worker's reason unless the owner's own message names the target; pay and destroy ask
  every time. Hard floor, undo hold, duplicate-send and contact checks unchanged. Owner data read in the request may
  only go to a target of it, something the owner named, or something they approved.
- `owner_ask`: a worker's question to the owner, shown in `ferb chat` with numbered options or a free-text prompt
  (`chat.answer`); declined at once during a scheduled run.
- Schedules record targets from the owner's words; a run is held to them.
- Deleted: the intent registry and verb tables, the reader label lists, the owner-browser phrase gate, the
  own-words typing rule, the spelling corrector, the route picker (the catalogue stays for the environment line).
- Tests: `test_policy`, `test_supervisor`, `test_routes`, `test_ux`, `test_gateway` rewritten; `test_explore` and
  `test_spelling` deleted; the acceptance set in `test_acceptance_one_worker.py` replays eight real failures.

### Mac
- Snapshots show what is really there: a control is anything that exposes an action (pressable rows, groups, cells
  and images included), with its actions, role, value and disabled / selected / expanded state; headings and the
  window title; an open sheet, dialog, pop-up menu or popover is in the snapshot with its place; the app's windows
  are listed (window server + the Accessibility routes that reach other-desktop windows) and one can be read by
  title or id; truncation says why. One tree walk per snapshot serves the snapshot, the before / after check and the
  password scan.
- Actions go background first, in the order the platform probes and the owner-verified runs established, each rung
  only after the one before verifiably did nothing: press → the enclosing control → focus + Return; type by keystrokes
  to the app's process (read back) → Accessibility keyboard where it has worked → the box's value; new `edit`
  (exact range through the text's selection, never arrow keys); a ⌘ shortcut runs its menu item and is refused with
  the Accessibility alternative while the app reports it disabled (it would only beep); escape cancels the open sheet
  / dialog / menu, never blind; new `scroll` (Page Up / Down with the area focused, then the scroll bar; sliders
  step); new `select` for pop-up buttons; click-at-point turns the point into the control under it first, posts a
  click to the app alone only for a window on the current desktop, and uses the real mouse only with
  `allow_foreground=true` — which is now the only way any `mac.*` call brings an app in front.
- What works per app is learned, not written: a per-app profile (`~/.ferb/device/apps/<bundle>.json`) counts each
  mechanism's hits and misses from every action, skips one after three misses in a row until it works again, and
  can be seeded by `ferb.act.mac.probe --seed`. The old per-daemon "ignores background keys" flag is gone.
- Full key table (letters, digits, F-keys, Home / End / Page Up / Down, forward delete, punctuation, keypad) and
  modifier aliases; ⌘Q / ⌘H / ⌘M refused like app switching.
- Real test: TextEdit and Calculator driven through the driver with the owner's frontmost app polled every 50 ms and
  never changing (`tests/test_mac_background_real.py`, `pytest -m real`).

## 0.16.1 — 2026-10-02
- Reminders: the planner's quote is checked against the words as typed. Before, the typo fix ran first, read "looks"
  as the Books app and dropped a test reminder. Word forms like "looks" and "minutes" now count as English.
  If a reminder is refused, FERB says so instead of showing a reply that claims it was set. (`2173352`)

## 0.16.0 — 2026-10-01 — Readers open what they need to read
- Readers may click to read on web pages: email rows, list items, tabs, and links and buttons with harmless labels.
  Forms and consequential labels are still refused. (`ff684ae`)
- Click-to-read counts as navigation, and a reader typing the owner's own words counts as a search, so a reading
  request's intent allows both. Security pages are checked first and still always ask. (`06299d6`)

## 0.15.1 — 2026-10-01
- Readers are told how to search an app. A refused reader press says what the reader can do instead. (`3d40f38`)

## 0.15.0 — 2026-10-01 — GPT-6 under Codex (spike 21)
- FERB gives Codex its own model catalog, with code mode, sub-agents and experimental tools switched off.
  The `functions.*` tool namespace is read as plain names. Model `gpt-6-sol`. Runtime safety tests pass on gpt-6-sol,
  gpt-6-astra and gpt-6-luna. (`baa7495`)

## 0.14.0 — 2026-10-01 — WhatsApp search in the background (spike 20)
- A reader may type the owner's own words into any box. (`9eb3844`)
- Search boxes that aren't real text fields are clicked and typed into with the app in front. set_value is used only
  on real text fields. (`c547901`)
- Background typing through the Accessibility keyboard (`AXUIElementPostKeyboardEvent`): WhatsApp search no longer
  needs the window in front. (`77f5491`, `f36bbb2`, `28f4897`)

## 0.13.0 — 2026-10-01 — Readers can open things
- Readers may click to read; the gateway keeps them to reversible kinds. The folder-access question comes for any
  folder named in the owner's words. (`0d352ac`)
- A running browser is woken through its binary. Search boxes count as searches. A step whose input is missing is
  skipped instead of run on an empty value. (`f773101`)
- Contract: the owner keeps the reader widenings, risk acknowledged. (`9315e24`)
- The reader instructions match what the gateway allows. (`8b8d5b9`)
- New `mac.menu` tool: an app's menus in the background. Readers may press only harmless items. Refused reader calls
  appear in the trace. (`1f44ab0`)

## 0.12.1 — 2026-10-01
- Docs brought up to date. (`8e45a80`)
- The workers' link to the daemon waits up to an hour. A slow reply is no longer reported as "the daemon went away".
  (`69fbbcd`)

## 0.12.0 — 2026-10-01 — The browser, profile and folder the owner names
- The browser the owner names is the one used: the planner is told, and the router enforces it. Extension install
  is guided. (`b0d2efd`)
- The owner decides what's appropriate for their own work; FERB does it in full. (`291f380`)
- Paths count by their folder names. Desktop, Documents and Downloads are handled, and so are Finder's ":" names.
  `ferb trace` shows every step. (`82451e2`, `8f72992`, `e9a0b12`, `18d119a`)

## 0.11.1 — 2026-10-01
- A task's verify and reconcile checks end with it, so their tabs no longer leak. (`2b7f8cb`)
- New `ferb voice on|off`. (`bbe9479`)

## 0.11.0 — 2026-10-01 — The browser wakes without moving the owner (spike 19)
- The browser starts with `open -g` and no startup window, and the first FERB window is created minimized. If the
  screen shifts anyway, FERB returns the owner to where they were. New paste switch,
  `[policy] pasted_text_is_data`. (`52c7278`)

## 0.10.0 — 2026-10-01 — Mac actions in the background (spike 18)
- Order of methods: Accessibility, then keystrokes posted to the app's process, then the foreground once per stretch
  of work, with the owner's input winning. Buttons are used over keystrokes. A second send to the same contact asks
  first. (`31d8a53`)
- Apps work from the background. The running-app list stays current. (`eb0c44d`)
- One close typo match is used and said; several are asked about. Plain reasons replace a vague apology. Read-only
  steps may open apps. Electron apps' Accessibility trees are switched on. (`009d05e`)

## 0.9.0 — 2026-10-01 — Voice
- Spikes 9 and 17: system audio taps, the microphone through Core Audio, on-device speech. (`8352ffc`, `5d26d4f`)
- Push-to-talk: hold Right Option. FERB.app is signed and runs in the background, and never uses a Bluetooth mic.
  (`2521022`, `15fe2ca`)
- `ferb voice`: a live view of what FERB heard and what it said. (`e1df2d7`)

## 0.8.0 — 2026-10-01 — Mac snapshots with a numbered image (Unit 2)
- `mac.snapshot` returns the window image with each control numbered (set-of-marks). (`5354065`)

## 0.7.0 — 2026-10-01 — Unit 1
- The owner stays on their desktop. FERB knows what it can do. Model-service errors are plain. The planner is faster.
  FERB can speak its replies (`--speak`). (`14e3e98`)

## 0.6.2 — 2026-10-01
- A loader in `ferb chat` while FERB works; plainer `ferb browsers`. (`1efdc6c`, `2060a07`, `efd729d`)

## 0.6.1 — 2026-09-30
- Several requests at once in `ferb chat`, later taken back to one at a time. FERB's own profile counts as available.
  A `[mac] yield_to_owner` switch. (`aacff6e`, `e42e795`, `ad587ef`)

## 0.6.0 — 2026-09-30 — Whose browser
- The owner's profile is used only when their words say so; otherwise FERB's own browser. FERB can take over an open
  tab and scroll the whole page. (`3652075`)
- Pasted requests and "yes" answers; `browser.find` searches the whole page; app aliases like "wa"; sites from
  history need site wording. (`7d3ff63`, `3611440`, `afbb25d`)

## 0.5.1 — 2026-09-30
- Docs: README, CODE_TOUR, CAPABILITIES. (`c1bf985`)

## 0.5.0 — 2026-09-30 — Exploring beyond the plan
- Workers may read outside the plan with a stated reason; acting elsewhere becomes a route change the owner approves.
  Real tests are locked down. (`87b91cb`)
- The planner's capability list is built in code from what's usable now. (`ca11c90`)

## 0.4.1 — 2026-09-30
- Screenshot clicks in messaging and calling apps declare their purpose. Real contact test. (`077c6c7`)

## 0.4.0 — 2026-09-30 — Contacts never guessed (build step 16a)
- The contact is checked against what was on screen, with a numbered "Which Rahul?" question. (`535d4d9`)

## 0.3.1 — 2026-09-30
- Structured times: the model says when, and code computes the date. Fuzzy time words, with the owner's corrections
  remembered. Service names that are everyday words count only in context. (`aff3e28`, `416baf5`, `4e1307f`)

## 0.3.0 — 2026-09-30 — Environment-aware resolution; calls and messages (build step 16)
- Routes are chosen in code: correction, then 14-day usage, then app, then paired web, then ask. Calls and messages
  get an undo window. (`871a010`)

## 0.2.0 — 2026-09-30 — Scheduled and recurring tasks (build step 15)
- Schedules are held to the recorded intent. Progressive setup. Time awareness. (`b413a38`, `2d501b7`)

## 0.1.0 — 2026-09-30 — §21 safety suite: all 24 pass
- Offline and real (Codex). Mailbox send is confirmed from the Sent folder. (`eacab8d`)

## 0.0.x — the foundation (2026-09-28 to 2026-09-30)

| Version | Build step / spikes | Commits |
|---|---|---|
| 0.0.16 | 14a: UX fixes from the owner's first session | `517c0af`, `cea1d01` |
| 0.0.15 | 14: durable timers and reminders, notifications, `time.*` tools, measured durations, mailbox send | `68b9d2c`, `6ee39b1`, `d50dc08` |
| 0.0.14 | 13: Jev in shadow mode | `ae638fa`, `6898536`, `dcb99c0`, `b5496b1` |
| 0.0.13 | 11: memory with provenance | `ff82446` |
| 0.0.12 | 10: watchers and the trail; budgets replaced by runaway guards | `4a698ba`, `fc96a7b`, `b0abac6` |
| 0.0.11 | 9: screenshot and click | `e544adc`, `b309ee1`, `2d6dc4d` |
| 0.0.10 | 8: multi-worker orchestrator | `1b58b45`, `94b7e40`, `3523d0d` |
| 0.0.9 | 7: Mac control | `b3f6a22`, `6200efd`, `e40e274` |
| 0.0.8 | 6b: FERB's identity on the extension, sealed secrets, inbox | `1da4387`, `419c67b`, `5372031` |
| 0.0.7 | 6a: extension, native host, pairing, owner `browser.*` tools | `c8a35b1` … `9376ecb` |
| 0.0.6 | 5: FERB's own browser, Keychain secrets, OTP; spike 16 | `6cae905` … `eca134d` |
| 0.0.5 | 4: runtimes, `ferb mcp`, gateway, files, `ferb chat` | `d595c25`, `702fe41` |
| 0.0.4 | 3: device, permissions, installer, setup / doctor; spike 15 | `7ed35dd` … `d8da763` |
| 0.0.3 | 2: sanitize, policy engine, ledger, `ferb stop` | `e862b3f` |
| 0.0.2 | 1: core, store, bus, clock, daemon, auth, journal, logs, trace | `33bcd0a`, `1e8041d` |
| 0.0.1 | Spikes 1–4, 6, 8, 10, 12–14 | `8d5802d` … `c4b0b07` |

## 0.27 F8 — checkout implementation, pending end checks and owner acceptance

- Added versioned derived memory, evidence cards, source-cascading forget, consolidation audit/rollback, scoped
  lessons, evidence-grounded procedure templates, learned/suspect/quarantined states and owner-only identity.
- Added before-plan candidates, read-only full memory retrieval, daily pending identity digest and a local dashboard.
- Added pinned local embedding preparation and owner-history retrieval evaluation; adoption remains off until
  measured improvement. Added paired with/without-memory report support without manufactured verdicts.
