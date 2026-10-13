# Launch-week fixes — design

Date: 2026-10-13. Branch: `astra_resume`. Status: owner's review answers applied (§8); awaiting approval.
Decisions here come from the owner's discussion on 2026-10-13; IDs L1–L24 are new (to be registered in
`docs/0.27_DECISIONS.toml` with milestone "L").

## 1. Why

Astra finished F5–F10 on `astra_resume` and the owner has used FERB daily through the native notch UI since the
2026-10-11 install. FERB launches publicly as an open-source repository and product on **2026-10-21**. The week before
is for the owner to use FERB heavily, mainly for the launch's social-posting campaign, and to find what else is rough.
So this work must land first and be small enough to leave most of the week for use.

The owner's complaints:
1. too many questions;
2. a late answer lands above the newer one, out of sight;
3. short answers that sometimes make no sense;
4. perception arrives as macOS notifications instead of in chat;
5. no instant acknowledgement or progress text in the UI;
6. notices restate what FERB saw instead of what it understood.

## 2. What the evidence showed

Five read-only reports, kept locally (git-ignored) in `.superpowers/sdd/2026-10-13-astra-review/`: `traces.md`,
`daemon-paths.md`, `ui.md`, `code-f5-f6.md`, `code-f7-f10.md`. Key facts:

- **Questions.** 25 tasks since install, all from the UI. They produced 100 approvals, all answered Yes, and 89 code
  confirmations.
  - 65 of the approvals were shell-network approvals, asked once per exact command text (`ferb/daemon/gateway.py:725-728`,
    `:1114-1146`). Task 93e9d7ec ("traction online") asked 46 times in 7 minutes.
  - That rule followed an owner choice made with Astra on 2026-10-11 ("approve opaque network shell commands once per
    exact command", `docs/OWNER_ACCEPTANCE.md:999`). It replaced the 2026-10-10 rule of once per host, only after
    outside content.
  - 13 more were URL-check false alarms. Context wrapper words such as "search" and "playlist" were treated as private
    payload, and every task starts tainted because context exposures count as outside content (`gateway.py:839-843`).
- **Ordering.**
  - The UI writes a late answer into its original turn while staying scrolled to the bottom
    (`ui/mac/FERBNotch/Sources/NotchCore/ConversationArchive.swift:66-73`, `ConversationTranscript.swift:16-45`).
  - `reply`, `clarify` and `reply_delta` events carry no id at all (`daemon-paths.md` §1.3).
- **Short answers.**
  - For unfinished tasks, code builds the short text from raw checker findings (`ferb/orchestrator/presentation.py:15-33`).
  - Follow-up corrections reuse the old goal (`supervisor.py:562-578`).
  - A partly refused reply is replaced whole (`supervisor.py:610-611`).
  - The planner never sees the screen. "What is this?" and "this sentient thingy" were answered from the chat topic.
  - In task fb80711c every image was withheld from the model, because of the browser and recording rule at
    `gateway.py:154-165`. FERB answered "where is this image from" without seeing it, after the screen had moved.
- **Acknowledgement.**
  - The daemon emits nothing until the planner returns, 10–52 s after the request. The P25 phrases exist only in the CLI
    client (`ferb/cli/live_chat.py:182-189`).
  - The plan line waits for the disturbance baseline (`supervisor.py:968-971`). The UI files it in a hidden activity feed
    (`DaemonConnection.swift:233-243`).
- **Perception.**
  - 33 notices came from 9,491 batches. Each was sent only as an osascript "FERB noticed" banner (`ferb/daemon/main.py:1628-1655`).
  - The attention prompt sees one raw batch with no owner context (`supervisor.py:1186-1213`).
  - Dedup never works: "already told" counts only acknowledged notices, and nothing acknowledges them. 22 of 33 notices
    repeat a subject.
  - FERB narrates its own clicks and reads its own banners back as notifications.
  - The Claude upgrade produced 4 alarm-style restatements.
- **Broken features.**
  - Learning:
    - the card redaction pattern corrupts memory IDs, so 35 task records failed to save;
    - all 24 consolidator runs failed;
    - the hourly memory loop has crashed since 2026-10-12 on `policy/security_digest.py:12-13`.
  - Typed-text capture records nothing (`ferb/sense/watchers/text_input.py:68-69`), which kills F9 owner-takeover
    detection.
- **Security bugs.**
  - The UI saves its draft unredacted in its preferences file (`NotchModel.swift:349`).
  - A digit answer to a 2FA-labelled question redacts that digit string everywhere until restart (`main.py:2556`,
    `ferb/sanitize/redact.py:93`).
  - The CLI auto-answers the only pending approval when the typed text matches an option, a payment included
    (`live_chat.py:176-182`).
- **Verdict on Astra's work.**
  - F5 security core (sandbox, proxy, escape hatch, screenshot redaction, sealed fills): reviewed and sound; keep.
  - F6 and F7: rework parts.
  - F8: good design, dead in practice; fix in place.
  - F9: fix, then verify live.
  - F10: not done (0 owner verdicts, 0 live security runs, null baseline after-values).
  - Task 5 round 4, T3b and everything from F6 on were never reviewed.

## 3. Owner decisions (2026-10-13)

| ID | Decision |
|---|---|
| L1 | Security direction: **trust, record, recover**. Prevention is phased: a small core this week, the full design after launch (§6). |
| L2 | **Shell and browser reach any site without asking.** Code records every site and lists it in the answer. Supersedes Q3 (2026-10-10) and the 2026-10-11 per-command choice. Accepted residual this week: after reading a malicious page, the shell could send one of the owner's files somewhere; this is recorded, not prevented. |
| L3 | **FERB asks only for:** money, account-security changes, permanent deletion, and sending as the owner to someone new unless a standing permission covers it (the launch posts). Each question shows the exact recipient and text, once per task, and the answer carries across retries. |
| L4 | Keep the private-data-in-links-and-sends check (S4), minus its false alarms. |
| L5 | Keep every protection that never asks: secrets as handles, credential stores locked, FERB's own files locked, sandbox, proxy recording, recovery copies. |
| L6 | Late answers appear **at the bottom, as a reply quoting the question**; the original spot says "answered below". |
| L7 | Instant acknowledgement (TQ3/P25) and the plan line appear in the UI chat. |
| L8 | While a task works, **one live line** under the plan line updates in place with the current step and elapsed time, and disappears when the answer arrives. Full steps stay under Details. |
| L9 | **Perception goes in chat, not macOS notifications** (owner: "it should show it in chat and not notifications"). |
| L10 | Jev still decides what reaches FERB (P11/Q4 unchanged). Once it reaches FERB, FERB understands before telling (§4.5). |
| L11 | Short answers use plain words from the owner's point of view and answer the question as asked (§4.4). |
| L12 | **The planner sees the front window with every message:** app, window title, and one redacted screenshot of that window taken **when the owner sends**. The worker for that message gets the same screenshot. |
| L13 | **Browser-window screenshots and recording frames go to the model, redacted, with no exceptions** (removes `gateway.py:154-165` withholding). |
| L14 | Fixes land on `astra_resume`. Afterwards it becomes `main` and the other branches are deleted (exact commands confirmed with the owner at that step). |
| L15 | The UI moved into this repository as `ui/` (done in 2b9f727; its old two-commit history is kept locally). |

L16–L24 are the fix items in §4.6, accepted by the owner as a list. L25 (§4.8) was added at approval.

## 4. Design

### 4.1 Questions and security (L2–L5)

**Shell network (L2).**
- Remove the `shell:command` network approval: the trigger at `gateway.py:725-728`, `_shell_approval`'s network
  reason, and the per-process grant check in `destination_allowed` (`gateway.py:458-484`).
- Any remote host is allowed through the proxy. These stay:
  - local services keep the executable-pinned permission;
  - the proxy keeps recording host, port and bytes (`shell.network` journal rows).
- The `external_source` approval ("run code declared as coming from outside content") goes too. Running outside code
  is recorded; quarantining it comes after launch (§6).
- Delete the dead Q3 code: `_reached_hosts`, `_outside_read` used only by it, `sni_enforced` (constant True),
  `_refused_hosts`, and `owner.ask(allow_destination)` (`code-f5-f6.md` I10), along with the tests that keep them alive.

**Sites in the answer.** The full answer (under Details, not the short text) ends with "Sites contacted:" followed by
hosts. Code builds this from the task tree's `shell.network` records and browser navigations, never the model.

**The ask list (L3).** The code floor stays as built: pay, account security, permanent delete, new recipient. On top
of it:
- **Bulk asks** (`gateway.py:845-856`): one question with count, size and a sample, as S4 says. Recipients and paths
  that appear in the owner's words are already authorized (rule 8) and don't ask.
- **Big-job plan (D24)** no longer waits for a Yes. As D30 already says for standing permission, the plan and any
  standing scope are stated in the acknowledgement and plan line. Work starts, and the owner corrects by replying, which
  becomes a steer.
  - This removes the blocking start question and its lost corrections (`supervisor.py:1047-1088`, I4).
  - Plan text published to the owner keeps D33 labelling.
- **D31 escape hatch** and **P19 retry after an unknown outcome** still wait. Both are rare: lifting a lock, and
  possibly double-paying or double-sending.
- **Worker prompts** no longer tell workers to ask for confirmation (`prompts/interaction.md:36-40`). Only code asks
  the floor questions. Workers bring other questions to the planner first (rule 19).

**S4 false alarms (L4).**
- Context exposures of FERB's own context (memory, history, device facts) count as outside content only when their
  stored origin is external. Today every task starts tainted (`gateway.py:839-843`, `supervisor.py:1597-1609`).
- `url_carries_private` matches only actual private values from the tree's private records, never FERB's context
  wrapper vocabulary.
- The check otherwise stays as built.

**Answers carry and don't get lost.**
- The planner routes free-text answers to open questions using their ids from its context. No exact-quote requirement,
  and no "/answer" or approval-id instructions to the owner (`supervisor.py:505-520`).
- When it can't tell which question an answer is for, it asks in plain words ("the payment or the email draft?").
- A plain "Yes" or "No" counts.
- The CLI's client-side auto-answer is removed (`live_chat.py:176-182`).
- **Steering never records a No** (`orchestrator/steering.py:101-110`). A pending confirmation stays open. If the steer
  changes the prepared action, the old confirmation is closed as "superseded", not denied.
- **The "Getting to know you" question** (`ferb/daemon/persona.py:57-68`) is created once. Status polling never
  recreates it, it never re-asks on expiry, and it sends no banner.

### 4.2 Chat flow (L6–L8)

**Request ids.**
- Every event about a message carries `request_id`, the id already journaled with `chat.message`, plus `task_id` where
  one exists. That covers `ack`, `plan`, `progress`, `reply`, `reply_delta`, `reply_abort`, `clarify`, `outcome` and
  `task.result`.
- `docs/RPC.md` is updated.
- Each event also carries an `at` timestamp.

**Acknowledgement (L7, TQ3).**
- On receiving `chat.send`, before the per-session routing lock, the daemon emits `ack {request_id, text}`.
- The phrase comes from the full owner-approved P25 list (all of it, not the 10 in `live_chat.py:13-14`), picked at
  random and never one of the last 8 used.
- It is journaled into chat history.
- The CLI shows the daemon's ack instead of its local one.
- P29's in-order routing per chat is kept. The ack doesn't wait for it.

**Plan line.**
- `plan` is emitted as soon as the planner returns, before the disturbance baseline sample (`supervisor.py:968-971`).
- Planning latency is measured. The measurement covers context building, which was up to 14 s, because recall and
  environment run synchronously (`supervisor.py:473-474`), plus the possible repair call.
- Avoidable waits are removed.
- `ferb baseline` gains the time from message to ack, plan line and answer.

**Live line (L8).**
- The daemon emits `progress {request_id, task_id, text, started_at}` when a worker starts a tool call.
- `text` is built by code from the tool and its target (for example "reading github.com", "looking at Brave", "writing
  notes.md"). It is never model text, so injected page text can't speak through it.
- The UI shows one line under the plan line with elapsed time and removes it when the outcome arrives.

**Late answers (L6).**
- When an outcome or reply arrives for a turn that isn't the newest, the UI does two things:
  - it appends a reply entry at the bottom with a one-line quote of that turn's question;
  - it marks the original turn "answered below".
- The answer stays linked to its request by `request_id`, so History shows both.
- `ContinuousChatTests` changes accordingly.

### 4.3 Screen context with every message (L12, L13)

**The capture.**
- On `chat.send` receipt, the daemon captures the front window, meaning the topmost window that isn't FERB's own. The
  notch takes keyboard focus while the owner types, so FERB's own windows must be skipped.
- What is captured:
  - app and bundle id;
  - window title;
  - for a browser, the front tab's URL and title;
  - one screenshot of that window only (D29).
- The screenshot goes through the F5 screenshot redaction. If redaction fails, the image is withheld, as today for Mail.

**Who gets it.**
- The planner's context for that message gets these as a labelled data block. Its origin is the window's content
  labels; it is data, never instructions (MP2).
- The worker started for this message gets the same screenshot as its start-of-task screenshot. This replaces D26's
  "taken when the worker first starts", so planner and worker both see what the owner saw.

**Withholding removed (L13).** `pixels_withheld` (`gateway.py:154-165`) no longer withholds browser windows or recording
frames. Redaction still applies.

**Answering directly.** The planner prompt says to answer a question about the screen directly from the screenshot
when it suffices: a reply with no worker and no checker. The Korean-line question took 1 min 16 s through planner,
worker and checker.

**Runtimes.** The planner model call must accept an image in every runtime adapter (D32: model-specific details stay in
the adapter).

**Privacy.** The hosted model sees the front window, redacted, with every message. The owner accepted this.

### 4.4 Answers (L11)

**Short text for unfinished tasks.**
- `outcome_display` stops building it from raw checker findings (`presentation.py:15-33`).
- The model writing the answer is given the checker's verdict and writes the short text from the owner's point of view:
  - what was delivered, with paths and links;
  - what isn't confirmed, in plain words;
  - what the owner can do, if anything.
- Code prepends the F4 status label ("Done, but not confirmed:" and so on).
- Full checker findings stay under Details.

**Prefix.** The literal "Verified. " prefix (`supervisor.py:1772`) is dropped. The UI shows verification as a small
check mark, and the CLI keeps its status line.

**Answer the question as asked.** The answer prompts say:
- the short text first answers the owner's question in the conversation's context;
- it includes the fact the thread is about (e.g. local vs cloud);
- it never uses internal terms ("report condition", exception names, "/answer", "open ferb chat").

The copyable example phrasings in `prompts/interaction.md:110-111` are removed.

**Follow-up corrections.** A `continue` control with a new goal on an ended tree replaces the goal and re-plans,
instead of reopening the saved plan with `prior_spec` (`supervisor.py:562-578`).

**Partial refusal.** When code refuses one side output (a timer, a memory write), the reply stays. One line reports
the refused part (`supervisor.py:610-611`).

**Reminder provenance.** "In the owner's own words" (`supervisor.py:271`) checks the owner-origin messages of this
chat, not only the current message. The 20:10 boarding timer the owner asked for at 19:52 should have passed.

**Steering replies** say what changed ("Switched to: make it a web page"), not "Updated the running task."
(`steering.py:115-117`).

### 4.5 Perception in chat (L9, L10)

**Delivery.**
- Attention notices become chat messages in the owner's daily conversation:
  - journaled as `chat.notice` with an id;
  - published as `attention.notice` with that id;
  - rendered by the UI and CLI as FERB messages with a small "noticed" tag.
- A notice posted to chat counts as told. That fixes "already told" (`main.py:1644`), which today waits for an
  acknowledgement that never comes.
- macOS banners (`main.py:1649-1650`, and task results at `main.py:1487-1498`) are sent only when no FERB client (UI or
  chat) is connected.

**One moment, one message.**
- At most one attention consideration runs per app at a time.
- Jev "yes" batches that arrive meanwhile, for any window or tab of that app, merge into the next consideration.
- This stops one event from becoming several near-duplicates, and adds no timing constant.

**FERB knows its own actions.**
- Events in an app window or tab while FERB was acting there, or in FERB's own browser profile, are recorded with actor
  FERB, as facts (P4).
- With banners gone, FERB no longer reads its own banners back (`code-f7-f10.md` I2).

**Understanding step.**
- The consideration prompt (`supervisor.py:1186-1213`) gets:
  - the merged rows;
  - the owner's relevant memory;
  - the recent chat;
  - running tasks;
  - the notices already told.
- It is asked for what happened and what it means for the owner, in FERB's words. For example: "You upgraded Claude to
  Max 20x. It renews on Nov 13."
- Outside text it quotes is returned separately and shown as a quote with its origin (MP2, I6).

**Correct the audit.**
- `trail.attention` is live per Q4. Its rows are recorded as live with their effect, and `ferb stats` reports them.
  Today they are mislabelled shadow (`questions.py:67-70`, C3).

**Other fixes.**
- No backlog at install: `pending()` gets a floor at the install or migration time (I1), so old trail rows don't flood
  Jev.
- Notification thread key and case-insensitive bundle-id mutes (I4).
- The D14 7-day purge also clears notice text derived from typed or clipboard rows (I3).

**Cost.** CPU and Jev call volume are measured before and after. D4 (an observer on every app) is unchanged. Further
cuts go to the owner as a proposal.

### 4.6 Broken features and bugs (L16–L24)

| ID | Fix |
|---|---|
| L16 | **Learning.** (a) Redaction never runs over internal ids: structured id fields (`source_refs`, node ids) are exempt, and the card pattern needs word boundaries so it can't match inside hex tokens (`redact.py:58,184`, `store/spans.py:49`). (b) Consolidator output schema made strict-compatible (`memory/consolidate.py:18-35`), with a test that constructs `Consolidator`. (c) `security_digest` accepts the string form of `network` (`policy/security_digest.py:12-13`); the hourly loop runs distill and consolidate in separate error scopes (`main.py:1295-1309`). |
| L17 | **Typed text.** Fix the PyObjC call (`text_input.py:68-69`). In the same change, close the password risk fixing it opens (I10): never capture secure text fields, input while Secure Input is on, or terminal apps (a `sudo` password typed in a terminal has no secure field and would otherwise reach Jev). No digit-shape rule: the owner rejected guessing codes by pattern (2026-10-13). |
| L18 | **2FA answer.** Only values entered through the sealed secret input become hidden secrets. Answers to choice questions never do (`main.py:2556-2558`, `redact.py:93-96`). |
| L19 | **CLI auto-answer** removed (`live_chat.py:176-182`); covered in §4.1. |
| L20 | **UI draft** is never written to preferences, only kept in memory (`NotchModel.swift:349`). Existing stored drafts are deleted on launch. |
| L21 | **Codex account errors** never drop a request silently. The request gets a chat reply naming the failure, so the owner can say retry. Check what 5b10afb already covers first. |
| L22 | **Waits after sleep** use the durable wall-clock timers (`clock/timers.py`) instead of `asyncio.wait_for` (`plan_executor.py:36-46`). |
| L23 | **Event loop.** The plan inventory walk (`plans.py:54-70`) runs off the loop (`supervisor.py:298-318`). |
| L24 | **Local services (T3b)** follow the owner's Q1 answer: direct connections to executable-pinned local ports are allowed and recorded, not only through the HTTP proxy (`sandbox.py:177-180`). |

**Launch campaign path.** The owner's real use this week is scheduled posting under a standing permission (D30). It
must work end to end:
- a standing permission stated in the acknowledgement;
- durable waits (L22);
- posts inside the scope that don't ask and are logged;
- anything outside the scope still asks.

It gets offline tests through the gateway (missing today, `code-f5-f6.md` I11) and an owner-approved live run on the
owner's own accounts.

### 4.7 UI (`ui/mac/FERBNotch`)

**Chat rendering.**
- Render `ack`, the plan line, the live line, late-answer replies and notices in chat.
- Stop filing the plan line in the activity feed. `ReplyStreamingTests:35-45` changes.

**Questions.**
- Questions appear inline in the chat. Typing stays enabled while one is open; today an open question replaces the chat
  (`NotchModel.swift:320-381`).
- The "Is this your request?" paste dialog is removed.

**Other changes.**
- Acknowledge notices and results the UI showed, so the CLI doesn't replay them (`attention.ack` and the result
  acknowledgement).
- Fix the `events.dropped`/`bus.dropped` topic name.
- Recovered tasks no longer show the daemon's goal text or "Previous task" as if the owner typed it.
- No draft persistence (L20).

**Build.** `./build.sh` in `ui/mac/FERBNotch`; the owner relaunches from `ui/`. The running copy still runs from the
old path until relaunched.

### 4.8 Extension auto-install at setup (L25)

The owner asked for this at approval, after seeing OpenClaw do it. OpenClaw's documented mechanism
(docs.openclaw.ai/tools/chrome-extension) works like this:
- it registers its native host;
- it asks Chrome to install its **Chrome Web Store** extension, and the request applies to every profile in that
  user-data directory;
- Chrome installs the extension at its next start and asks the owner to enable it in each profile;
- OpenClaw never restarts the browser or approves the prompt.

FERB does the same.

**What `ferb setup` does** (and `ferb setup --update`), for every installed Chromium-family browser whose user-data
directory exists (Chrome, Brave, Edge, Chromium, Vivaldi, Arc, Opera and others the code finds):
- writes the Web Store install request;
- registers FERB's native host;
- reports, per profile, whether the extension is installed, enabled and paired. It reads Preferences only, never
  writes them.

Setup doesn't restart a browser or approve its prompt. Setup output says which browsers must be reopened to pick
the extension up.

**Dependency (owner action).** The extension must be published on the Chrome Web Store. FERB prepares the package and
the listing text. Until the Store ID exists, setup keeps the current Load-unpacked steps. The ID is configurable, so
setup works the day the listing goes live.

The native host accepts both the pinned unpacked ID and the Store ID.

**To verify in implementation:**
- off-store automatic install (a local CRX) appears to be blocked on macOS without enterprise management;
- the install request may not work in some browsers (e.g. Brave, Arc).

Where it doesn't work, setup reports it and keeps the manual path. It never edits a browser's Secure Preferences.

## 5. Changes to the plan of record and working rules

Per CLAUDE.md rule 4, these conflicts are named here. Approving this spec approves the updates.

- **Q3 (2026-10-10), the 2026-10-11 per-command choice, and CLAUDE.md rule 8's sink rule as it applied to shell
  egress:** replaced by L2. S4 still covers sends and URLs.
- **CLAUDE.md rule 9:** a big job's plan (D24) no longer waits. It is stated and correctable (§4.1). D31 and P19 still
  wait.
- **D6/D26:** the screenshot is taken at send time with every message and given to the planner. The worker reuses it
  (L12).
- **D29:** browser windows and recordings may go to the model, redacted (L13).
- **P11/Q4 delivery:** chat, not banners (L9).
- **TQ3/P25:** the acknowledgement comes from the daemon, using the full list (L7).
- **CLAUDE.md rule 1:** work is on `astra_resume`, later `main` (L14).
- **Docs to update:**
  - `docs/0.27_PLAN.md`: dated clarifications;
  - `docs/0.27_DECISIONS.toml`: L-series entries;
  - `docs/RPC.md`, `ARCHITECTURE.md`, `README.md` and `CHANGELOG.md`;
  - `AGENTS.md`: "Where things stood" is stale, and its current uncommitted change is kept and built on;
  - `ui/AGENTS.md` where its chat rules change.

## 6. After launch (phase 2, not built this week)

The fuller design discussed on 2026-10-13, kept so nothing is lost:
- trust by author, not storage location (inbox, downloads and pages are outside even when local);
- following a link versus building one (model-built requests carrying sensitive data get checked);
- per-task folder scope, set from the owner's words, which closes the L2 residual;
- tagging memory written after outside content (it can't supply a path or destination until the owner confirms it);
- outside code quarantined;
- links from outside content open logged out;
- writes scoped to task folders, with script-type files asking after outside content;
- "authorize this app" pages counted as account security;
- sensitive-by-content files, including Indian ID formats, added with care given L16(a);
- trusted contacts narrowed;
- a delegation mode;
- send approvals bound to the exact text.

Also deferred:
- map coverage checked against real reads (P15, `code-f5-f6.md` I7);
- F9 thread matching (I5);
- the remaining code-health items.

## 7. Testing and acceptance

**Offline.**
- A regression test first for every behaviour fix (CLAUDE.md).
- Golden replays built from the traced cases:

  | Traced case | Expected |
  |---|---|
  | 93e9d7ec research | 0 questions |
  | the Claude-upgrade rows | one chat notice |
  | f5fa5baf/29dfecc7 ordering | UI test |
  | ef9e5d75 "too linkedinish" | the new goal is used |
  | the 20:10 partial refusal | the reply is kept |
  | "what does this Korean line mean" with a screenshot | a planner reply |
  | the 2FA "1" answer | not redacted |
  | card-like digits inside node ids | not redacted |

- Standing permission through the gateway.
- Waits across a simulated sleep.
- Then `uv run pytest -q`, `uv run mypy ferb/core`, `git diff --check`, and `swift test` in `ui/mac/FERBNotch`.

**Process.** Every task gets a fresh reviewer. This work exists partly because F6 onward went unreviewed.

**Install.** The owner runs `ferb setup --update` and rebuilds the UI.

**Live (the owner's week).** `ferb verdict TASK pass|fail` as the owner goes. Checks:
- the ack appears at once;
- Sentient-style research asks nothing;
- a screen question is answered from the screenshot;
- a late answer appears at the bottom;
- a subscription-like event gives one understood chat notice;
- the launch posting runs under standing permission (owner-approved);
- learning writes episode cards and the hourly loop runs;
- a Zomato login with an OTP through sealed filling (§8.6).

Results go into `docs/OWNER_ACCEPTANCE.md`.

## 8. Owner answers at review (2026-10-13)

1. **Question expiry:** none. A question stays open while its task waits (replaces the 600 s expiry,
   `daemon/questions.py:10`).
2. **D31 and P19 keep waiting for the owner** (§4.1).
3. **"Verified."** becomes a check mark (§4.4).
4. **macOS banners only when no FERB client is connected** (§4.5).
5. **No digit-shape OTP rule** in typed text (L17).
6. **Sealed filling stays as built** for codes and passwords (D8/D12/D28, rule 14 unchanged). The owner considered
   removing it after the Zomato OTP failures, then chose to keep it and test it after these fixes:
   - Zomato failed on 10-04, 10-11 00:30 and 10-11 14:16 because `browser.login_code` refused the split code boxes.
   - On 10-11 14:16 the secret helper's signature check also failed.
   - Astra's split-box fix (e14d691, 10-11 15:06) and the helper reinstall (10-11 15:38, verifies today) both came
     after the last attempt and are untested live.

   Live check: the owner runs a Zomato login with an OTP after install.
