# Launch-week fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix what the owner's live use of F5–F10 exposed (questions, chat flow, answers, screen context, perception,
broken learning/typed text, security bugs, UI), and add extension auto-install at setup, before the 2026-10-21 launch.

**Architecture:** Daemon changes stay in their layers: `ferb/daemon/gateway.py` (boundaries), `ferb/orchestrator/*`
(planning, routing, answers), `ferb/daemon/main.py` (RPC/streams, attention delivery), `ferb/sense/*` (perception),
`ferb/sanitize/redact.py`, `ferb/memory/*`. The UI (`ui/mac/FERBNotch`) consumes the event contract below. Work runs
in six parallel lanes; each task is one implementer in its own git worktree, reviewed, then cherry-picked onto
`astra_resume`.

**Tech Stack:** Python 3.12 (uv, pytest, mypy), SwiftPM (FERBNotch), Chromium extension (MV3) and native messaging.

**Spec:** `docs/superpowers/specs/2026-10-13-launch-week-fixes-design.md` (L1–L25; §8 owner answers). Evidence
reports (local, read-only): `.superpowers/sdd/2026-10-13-astra-review/{traces,daemon-paths,ui,code-f5-f6,code-f7-f10}.md`
under `/Users/kartikey/Desktop/FERB`.

**Plan form (ruling):** the owner asked to "quickly start heavy parallel coding". Tasks therefore specify behaviour,
files, interfaces and exact test cases rather than full code. Implementers read the cited code and reports, and write
each listed test first (RED), then the implementation (GREEN).

## Global Constraints

- Work only in your task's worktree. Never touch `vibed/`, `~/.ferb` (installed state), the running daemon, or
  personal browsers. Never read or write real secrets.
- CLAUDE.md applies, as amended by the spec's §5:
  - code enforces where/what boundaries and never judges intent by keyword or label lists (rule 6);
  - secrets and OTPs never reach a model, trace, log or the database (rule 14, unchanged: sealed fills stay, §8.6);
  - trust follows origin (MP2);
  - background first (rule 17).
- Asks that remain: money, account-security change, permanent deletion, send as the owner to a new recipient without
  standing permission (code floor), the S4 sink check for sends/URLs, D31 escape hatch, P19 retry after an unknown
  outcome. Nothing else waits for the owner. Questions never expire while their task waits.
- **Introduce no new numeric limit, cap, budget, retention or timeout** that changes FERB's behaviour (owner rule). If
  one seems necessary, stop and report NEEDS_CONTEXT.
- Every behaviour fix starts with a failing regression test.
- Validation per task:
  - focused tests while iterating;
  - before committing, once: `uv run pytest -q -p no:cacheprovider`, `uv run mypy ferb/core`, `git diff --check`;
  - Swift tasks: `swift test` in `ui/mac/FERBNotch`.
- Commit messages cite spec IDs (L1–L25) and the plan-of-record IDs they touch, and end with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Docs: update `docs/RPC.md` when you change an RPC/event shape. Leave the other docs to Task 19 (`ARCHITECTURE.md`,
  `README.md`, `CHANGELOG.md`, `docs/0.27_PLAN.md`, `docs/0.27_DECISIONS.toml`, `AGENTS.md`).

### Event contract (Tasks 4, 5, 6, 10, 11, 15, 16 build against this exactly)

Every event on a `chat.send` stream and the matching bus topic carries `request_id` (the id journaled with that
`chat.message`) and `at` (ISO 8601 with offset). Events about a task also carry `task_id` (the tree id).

| event | fields (besides request_id, at) | when |
|---|---|---|
| `ack` | `text` | on receipt, before routing (P25 phrase) |
| `plan` | `task_id`, `text`, `goal` | planner returned a task, before the disturbance baseline |
| `progress` | `task_id`, `text`, `started_at` | a worker starts a tool call; `text` is built by code from tool + target |
| `reply`, `clarify` | `text`, `display_text` | as today |
| `reply_delta` / `reply_abort` | `delta` / — | as today |
| `outcome` | `task_id`, `status`, `answer`, `display_text`, `verified` (bool) | task finished |
| bus `task.result` | same as `outcome` | as today, plus the new fields |
| bus `attention.notice` | `id`, `text`, `quotes` (list of `{text, origin}`), no `request_id` | a notice is posted to chat |
| `approval.requested` | add `request_id` when the approval belongs to a request | as today otherwise |

`chat.status` returns `notices` (unacknowledged, each `{id, text, quotes, at}`). `attention.ack {ids}` marks them seen.

## Review Focus

1. Two requests in flight: their acks, plan lines, progress and answers each carry their own `request_id`. Nothing
   attaches to the wrong turn (Tasks 4, 5, 15).
2. An open question while the owner keeps chatting:
   - a typed "Yes" answers the right question;
   - a steer never records a No;
   - ambiguity gets a plain-words question, never "/answer" (Task 7).
3. Removing shell approvals must not open FERB's own state, the Keychain, or local services without the executable
   pin (Task 1).
4. Screen capture at send must skip FERB's own windows. It must work when the front app is a browser, and must
   withhold the image only when redaction fails (Tasks 3, 8).
5. Redaction:
   - no internal id is ever altered;
   - a short choice answer ("1") never becomes a secret;
   - sealed values are still redacted everywhere (Tasks 12, 13).

## Lanes and order

| Lane | Tasks (in order) | Owns |
|---|---|---|
| G: gateway & policy | 1 → 2 → 3 → 14 | `daemon/gateway.py`, `policy/*`, `act/ask.py`, `act/shell/*`, `daemon/questions.py`, `daemon/persona.py` |
| C: chat & answers | 4 → 5 → 6 → 7 → 8 | `orchestrator/supervisor.py` (except the attention prompt), `presentation.py`, `steering.py`, `reply_stream.py`, `daemon/main.py` chat-send region, `cli/live_chat.py`, prompts |
| P: perception | 9 → 10 | `sense/*`, attention region of `main.py` and `supervisor.py`, `daemon/questions.py` (`trail.attention` row only) |
| B: broken features | 11 → 12 → 13 | `sanitize/redact.py`, `store/spans.py`, `memory/*`, `policy/security_digest.py`, `sense/watchers/text_input.py`, `plan_executor.py`, `plans.py`, runtime account errors, `main.py` distill loop |
| U: UI | 15 → 16 | `ui/mac/FERBNotch` |
| X: extension | 17 → 20 | `ferb/act/browser/*` setup/install, `ferb/cli/*` setup, extension bundle |
| R: release | 21 | `scripts/`, app bundle, install paths |
| Final | 18 → 19 → whole-branch review | tests through the gateway, docs |

Task 18 starts after Tasks 1, 7 and 13 are integrated; Task 19 after all others.

---

### Task 1: Shell and browser reach any site without asking; sites recorded (L2)

**Files:**
- Modify:
  - `ferb/daemon/gateway.py`: `:725-728` (the trigger), `_shell_approval` `:1114-1146`, `destination_allowed`
    `:458-484`, and the dead `_reached_hosts`/`_outside_read`/`sni_enforced`/`_refused_hosts` at `:399-502`;
  - `ferb/act/ask.py:25-27` (`allow_destination`);
  - shell process network flag plumbing where `proc_network` is set.
- Create: `ferb/policy/sites.py`.
- Test: `tests/test_shell_network_free.py` (new); update or remove `tests/test_sinks.py:162,202` and the tests of the
  removed approval.

**Interfaces:**
- Produces: `ferb.policy.sites.sites_contacted(spans, tree_id: str) -> list[str]`. It returns sorted unique hosts this
  tree reached. Sources:
  - `shell.network` records (the same data `ferb timeline` shows as `shell.network ... destinations=[{host,...}]`);
  - browser navigations of the tree's own tabs.
- Task 6 consumes it.

**Behaviour:**
- A `shell.run`/`shell.start` with `network=true`, or with `external_source`, runs with no confirmation and no
  `shell:command` approval row.
- The proxy allows any remote host for a process FERB tracks. It still:
  - fails closed for an untracked process;
  - requires the executable pin for loopback/localhost;
  - records host, port and bytes as today.
- No tree-level host No exists any more, because nothing asks.
- `owner.ask(allow_destination)` is removed from the model-facing schema.
- The sandbox profile, Keychain denial and FERB-state protections are unchanged.

- [ ] Write tests:
  - (a) a networked shell command runs without `ask_confirmation` being called;
  - (b) `destination_allowed(proc, "github.com", 443)` → allowed for a tracked process; untracked → refused;
    `127.0.0.1:5432` without a pin → refused;
  - (c) `external_source` set → runs, and the span records `external_source`;
  - (d) `sites_contacted` dedups and sorts hosts from two processes and one browser navigation;
  - (e) the never-read protections still refuse `~/.ferb` and the Keychain through the shell.

  Run them; they fail.
- [ ] Implement; delete the dead Q3 code and its tests.
- [ ] Focused tests, then the full suite, mypy and diff-check; commit `fix(L2): shell reaches any site, recorded (Q3 superseded)`.

### Task 2: S4 false alarms and bulk asks (L3, L4)

**Files:**
- Modify:
  - `ferb/daemon/gateway.py:839-856` and the `url_sink` path `:930-975`;
  - `ferb/policy/confirm.py` (`url_carries_private`, the bulk/S4(c) decision);
  - `ferb/orchestrator/supervisor.py:1597-1609`. Exposure origins only; keep this hunk minimal, since lane C owns the
    rest of the file.
- Test: `tests/test_sink_false_alarms.py` (new).

**Behaviour:**
- A context exposure counts as outside content only when its stored origin is external. FERB's own memory, history
  and device facts don't taint a tree.
- `url_carries_private` matches only actual private values from the tree's private records, never FERB's context
  wrapper vocabulary.
- Bulk confirmations (S4(c): permanent directory delete, send to 2 or more recipients):
  - recipients and paths that appear in the owner's words are authorized and don't ask;
  - otherwise one question shows count, total size and a sample of up to 3 items. The 3 is display only; it changes no
    behaviour.

- [ ] Tests from the traced cases (`traces.md` §2): a Google `/search?q=…`, a YouTube `playlist?list=…`, a `history`
  URL, and a local `index.html` all reach the browser with no question in a tree that read a web page. A URL carrying a
  real private value (an email from a private record) to a host nobody named still asks. A permanent delete of an
  owner-named folder doesn't ask. A send to two owner-named recipients doesn't ask. A send to 5 un-named recipients
  asks once, with count and a sample of 3. All fail first.
- [ ] Implement, validate, commit `fix(L3,L4): S4 without false alarms; one bulk ask with a sample`.

### Task 3: Browser windows and recording frames reach the model, redacted (L13)

**Files:**
- Modify: `ferb/daemon/gateway.py:154-165` (`pixels_withheld`) and its callers.
- Test: `tests/test_pixels_to_model.py` (new); update existing withholding tests.

**Behaviour:**
- `pixels_withheld` no longer withholds browser-app windows, web sources without a host, or recording frames.
- An image is withheld only when the F5 sanitizer/redaction fails, as today for Mail.
- `recording.inspect` returns frames to the model.

- [ ] Tests:
  - `mac.screenshot` of a Brave window keeps its pixels and goes through redaction;
  - `recording.inspect` returns sample frames;
  - a forced sanitizer failure still withholds.

  All fail first.
**Added 2026-10-14 (owner's task 252d2a9d: FERB recorded a reel but never got it playing, and couldn't see it):**
- `browser.screenshot` no longer refuses a page because its snapshot was truncated or it has secret fields
  (`ferb/act/browser/ext_bridge.py:1059-1061`). It captures with every secret field masked, using the field rects the
  extension reports for secret fields, queried directly rather than from the truncated element list. It withholds the
  image only when masking fails.
- **No screenshot or recording is refused anywhere** (owner, 2026-10-14: "no screenshots should be refused and go with
  redacted stuff"). That includes:
  - native window captures of a browser that has a password field, which today `background.js` `window_password`
    refuses through `ferb/act/mac/driver.py:295,336`, and which also misses shown passwords;
  - `browser.screenshot`;
  - `recording.*` frames.

  Each is captured with every secret field masked. For a browser window the rects come from the content script's
  `isSecret`/`passwordField` (the same fields the 0.15.3 bundle marks secret). For native apps they come from the AX
  secure-field rects, now also covering AX password elements. An image is withheld only when the masking step itself
  fails.
- Tests: a truncated snapshot with a secret field yields a masked screenshot, not a refusal; a native capture of a browser
  window with a shown password has that field masked.

- [ ] Implement, validate, commit `fix(L13): browser windows and recordings reach the model, redacted (D29 clarified)`.

### Task 4: Event contract, daemon acknowledgement, no CLI auto-answer (L7, L19)

**Files:**
- Modify:
  - `ferb/daemon/main.py:2579-2636` (`_chat_send`);
  - `ferb/orchestrator/supervisor.py:418-490` (send/intake);
  - `ferb/cli/live_chat.py:12-14,176-189`;
  - `docs/RPC.md`.
- Create: `ferb/orchestrator/acks.py`, holding the full owner-approved P25 list copied verbatim from
  `docs/0.27_PLAN.md:822-829`, minus the starred "On it, one sec."
- Test: `tests/test_chat_events.py` (new).

**Interfaces:**
- Produces:
  - `acks.pick(recent: list[str]) -> str`: random, never one of `recent[-8:]`;
  - the event contract above (all fields).

**Behaviour:**
- `_chat_send` emits `ack {request_id, text, at}` before taking the per-session routing lock, and journals it as
  `chat.ack`.
- Every event the turn yields carries `request_id` and `at`. Task events also carry `task_id`.
- The CLI prints the daemon's ack instead of its local one.
- The CLI never auto-answers an approval from typed text; the old `live_chat.py:176-182` path is deleted.

- [ ] Tests:
  - (a) the ack arrives before the planner is called. Use a planner stub that blocks until released and assert that
    the ack was received first;
  - (b) two concurrent sends on one session each get their own ack immediately, and each event carries its
    `request_id`;
  - (c) `pick` never repeats one of the last 8 over 200 draws;
  - (d) the CLI with one pending approval and typed "Yes" sends `chat.send`, not `chat.approve`.

  All fail first.
- [ ] Implement, validate, commit `feat(L7,L19): daemon ack and request ids on every chat event (TQ3, P25)`.

### Task 5: Plan line first, live progress line, latency in baseline (L8)

**Files:**
- Modify:
  - `ferb/orchestrator/supervisor.py:908-1045` (`run_tree`: yield `plan` before `on_task_start`'s baseline wait at
    `:968-971`);
  - `:473-474` (recall/environment: run without blocking the loop);
  - `ferb/orchestrator/worker.py:131,169` (tool start hook);
  - the `ferb baseline` measurement code.
- Create: `ferb/orchestrator/progress.py`.
- Test: `tests/test_progress_line.py` (new).

**Interfaces:**
- Produces: `progress.describe(tool: str, args: dict) -> str`. It is code-built and never uses model text. Examples:
  - `browser.read` on `https://github.com/x` → `reading github.com`;
  - `mac.snapshot` app Brave → `looking at Brave`;
  - `fs.write` path `/a/b/notes.md` → `writing notes.md`;
  - `shell.run` → `running a command`.
- Unknown tools → the tool's verb from its registry kind (W2).

**Behaviour:**
- The `plan` event is emitted as soon as the planner returns.
- A `progress {request_id, task_id, text, started_at}` event is emitted at each worker tool start.
- `ferb baseline` records message→ack, message→plan and message→answer times.

- [ ] Tests:
  - the plan event precedes the baseline sample (stub `on_task_start` to block);
  - progress events are emitted with code-built text for 5 tool kinds;
  - a `why` field containing injected text never appears in `progress.text`.

  All fail first.
- [ ] Implement, validate, commit `feat(L8): plan line before the baseline; code-built live progress`.

### Task 6: Answers in the owner's words (L11)

**Files:**
- Modify:
  - `ferb/orchestrator/presentation.py`;
  - `ferb/orchestrator/supervisor.py:1744-1816` (`_final`) and `:1772` ("Verified. " prefix);
  - `:562-578` (continue with a new goal);
  - `:584-616` (side outputs) and `:610-611` (partial refusal);
  - `:271` (reminder provenance);
  - `ferb/orchestrator/steering.py:115-117`;
  - `ferb/orchestrator/prompts/interaction.md:110-111` and the worker/supervisor answer instructions.
- Test: `tests/test_answers_owner_words.py` (new).

**Interfaces:**
- Consumes: `sites_contacted` (Task 1).
- Produces: `outcome.verified: bool`.

**Behaviour:**
- **Unfinished-task short text.** For failed, needs_user and done_unverified, the short text is written by the model
  that writes the answer. It gets the checker verdict and findings as input and states, in plain words:
  - what was delivered (paths, links);
  - what isn't confirmed;
  - what the owner can do.

  Code prepends the F4 status label. Findings stay in the full answer. Code never puts raw findings in `display_text`.
- **Verified prefix.** The "Verified. " prefix is removed, and `verified=true` is set instead.
- **Sites contacted.** The full answer ends with `Sites contacted: host, host` (code-built) when the list isn't empty.
- **New goal.** A `continue` control with a new goal on an ended tree replaces the goal and re-plans, instead of using
  `prior_spec`.
- **Partial refusal.** A refused side output (timer, memory) no longer replaces the reply. One line reports the refused
  part.
- **Reminder provenance.** "In the owner's own words" accepts owner-origin messages in this chat's history, not only
  the current message.
- **Steering replies** name what changed: "Switched to: <new goal>".
- **Prompts:**
  - remove the copyable example phrasings;
  - require the short text to answer the question as asked, with the thread's key fact;
  - forbid internal terms.

- [ ] Tests, from the traced cases in `traces.md` §4:
  - (a) a needs_user outcome's `display_text` contains no checker finding text verbatim and starts with the status
    label;
  - (b) a verified outcome has `verified=True` and no "Verified." prefix;
  - (c) ef9e5d75's shape: continue with a new goal runs the new goal;
  - (d) 20:10's shape: reply plus refused timer → reply kept plus one refusal line;
  - (e) a timer the owner asked for in an earlier message of the same chat passes provenance;
  - (f) a steer reply names the new goal;
  - (g) "Sites contacted" lists Task 1's hosts.

  All fail first.
- [ ] Implement, validate, commit `fix(L11): answers in the owner's words; follow-ups re-plan; partial refusals kept`.

### Task 7: Answers carry, steering never refuses, plans don't block (L3, §8)

**Files:**
- Modify:
  - `ferb/orchestrator/supervisor.py:505-520` (answer routing) and `:1047-1088` (plan question);
  - `ferb/orchestrator/steering.py:101-110`;
  - `ferb/orchestrator/prompts/interaction.md:36-40` and `supervisor.md`;
  - `ferb/daemon/gateway.py` (confirmation state `superseded`; coordinate with lane G by touching only the state
    handling at `:1048-1058`).
- Test: `tests/test_owner_answers.py` (new).

**Behaviour:**
- **Free-text answers.** The planner returns `answer_question {approval_id, answer}`, using ids from its context. Code
  accepts any open `owner.ask`/`plan.ask`/`ferb.confirm` by id. No exact-quote check. No "/answer" text anywhere.
- **Plain Yes/No.** A plain "Yes"/"No" answers when the planner picks the question.
- **Ambiguity.** When it's ambiguous, the planner asks in plain words.
- **Steering.** It never resolves pending confirmations as denied. If the steer changes the prepared action, the
  pending confirmation closes with state `superseded`, which is not a No and doesn't make the action final-refused.
- **Big-job plans (D24).** They no longer wait for a Yes. The plan outline and standing scope are published as a
  D33-labelled message (`confirm.task_question`/`from_task_lines`) and the work starts. A reply becomes a steer.
- **Worker prompts** no longer tell workers to ask for confirmation. Workers bring questions to the planner.

- [ ] Tests:
  - (a) an open payment confirmation plus a planner clarify, then owner "Yes" routed by id → only the identified one
    resolves;
  - (b) a steer during a pending new-recipient confirmation → state `superseded`, and a later identical action asks
    again rather than being refused;
  - (c) a D24 plan publishes and starts without an approval;
  - (d) no owner-facing text contains "/answer";
  - (e) prompt contract: interaction.md has no confirmation-asking instruction for workers.

  All fail first.
- [ ] Implement, validate, commit `fix(L3): answers by id, steering never refuses, plans stated not blocking (D24, D30, D33)`.

### Task 8: Front window with every message (L12)

**Files:**
- Modify:
  - `ferb/daemon/main.py` (`_chat_send` capture before routing);
  - `ferb/orchestrator/supervisor.py:644-731` (planner context block plus image);
  - the runtime adapters under `ferb/runtimes/` (planner image input for Codex CLI, Claude SDK and direct APIs, D32);
  - the worker start-of-task screenshot path (D26);
  - `prompts/supervisor.md`.
- Create: `ferb/sense/front_window.py`.
- Test: `tests/test_front_window_context.py` (new).

**Interfaces:**
- Produces: `front_window.capture() -> FrontWindow | None`.
- `FrontWindow(app, bundle_id, title, url, tab_title, image_ref, origin)` describes the topmost on-screen window
  whose owner isn't FERB (the notch app, FERB's own profile windows are FERB's actions but still visible: skip only
  FERB processes).
- `image_ref` is an evidence-store reference to the redacted window screenshot. It is None when redaction fails.

**Behaviour:**
- On every `chat.send`, the capture runs before routing.
- The planner gets a labelled data block (app, title, URL, tab title) and the image.
- The worker started for that request uses the same image as its start-of-task screenshot, with no second capture.
- The prompt says to answer screen questions directly from the image when it suffices.
- A capture failure never blocks the message.

- [ ] Tests:
  - (a) with FERB Notch frontmost above Brave, capture returns Brave;
  - (b) the planner context contains the block and an image for every message;
  - (c) the worker for that request reuses `image_ref` (no new capture call);
  - (d) a redaction failure gives `image_ref=None` but keeps the text facts;
  - (e) each runtime adapter accepts an image on a planner call (adapter unit tests with fakes).

  All fail first.
- [ ] Implement, validate, commit `feat(L12): planner sees the front window at send (D6/D26 clarified)`.

### Task 9: Perception notices in chat (L9)

**Files:**
- Modify:
  - `ferb/daemon/main.py:1487-1498` (task-result banners) and `:1628-1655` (`_consider_attention` delivery);
  - `ferb/daemon/questions.py:67-70` (`trail.attention` live);
  - `ferb/sense/batches.py:71-73` (install floor) and `:117-124`;
  - `ferb/sense/watchers/notifications.py:31,44` (thread key `thread`/`threadIdentifier` per the real record, read
    incrementally);
  - `ferb/sense/notifications.py:29` (case-insensitive bundle ids);
  - `ferb/sense/perception.py:77-88` (purge derived notice text);
  - event-actor tagging where trail events are recorded.
- Test: `tests/test_notices_in_chat.py` (new).

**Behaviour:**
- **Delivery.**
  - A notice is journaled `chat.notice {id, text, quotes}`, published on `attention.notice`, and listed in
    `chat.status.notices` until acked.
  - Posting to chat makes it "told".
  - osascript banners (notices and task results) are sent only when no FERB client is connected.
- **Audit.** `trail.attention` decisions record `mode='live'` with their effect.
- **Install floor.** `pending()` ignores trail rows older than the perception install/migration time.
- **Actor tagging.** Trail events in an app/tab while FERB has an action in progress there, or in FERB's own browser
  profile, get actor FERB.
- **Notification fixes.** Thread id from the real Notification Center record key; mutes compare bundle ids
  case-insensitively.
- **Purge.** D14's 7-day purge also clears notice text derived from typed/clipboard rows.

- [ ] Tests:
  - a notice reaches the chat journal and `chat.status`, and no osascript call happens while a client is connected;
  - with no client, the banner is sent;
  - the second consideration sees the first notice in "already told" without any ack;
  - pre-install rows are not batched;
  - a FERB-action-window event gets actor FERB;
  - a `net.whatsapp.whatsapp` row is muted by a rule saved as `net.whatsapp.WhatsApp`;
  - a real-shape NC record (fixture from the live key names) yields its thread id.

  All fail first.
- [ ] Implement, validate, commit `fix(L9): perception notices in chat; told without acks; live audit (P11, Q4, P4)`.

### Task 10: FERB understands before telling (L10)

**Files:**
- Modify: `ferb/orchestrator/supervisor.py:1186-1213` (attention prompt and schema; lane P owns this region) and the
  consideration scheduling in `ferb/daemon/main.py`.
- Test: `tests/test_attention_understanding.py` (new).

**Behaviour:**
- **One consideration at a time per app.** At most one consideration runs per app bundle. Jev "yes" batches for that
  app that arrive meanwhile merge into the next consideration. There is no timer.
- **Prompt input:**
  - the merged rows;
  - the owner's relevant memory (existing recall);
  - the last chat turns (existing history layer);
  - running tasks;
  - notices already told.
- **Prompt output.** It asks what happened and what it means for the owner, in FERB's words. It returns `tell`,
  `text`, and `quotes: [{text, origin}]` for any outside text it repeats.
- **No restating.** `text` must not restate raw rows. Enforce this through the prompt plus the quote separation; there
  is no code content filter.

- [ ] Tests:
  - the Claude-upgrade fixture (4 near-simultaneous batches from Brave windows, built from the shapes in `traces.md`
    §5) → exactly one consideration and one notice;
  - the consideration input contains memory, chat and told-notice blocks;
  - outside text appears only in `quotes` with its origin;
  - a second batch arriving during a consideration is merged, not dropped.

  All fail first.
- [ ] Implement, validate, commit `feat(L10): one moment one notice, with owner context and marked quotes (P11, MP2)`.

### Task 11: Learning works again (L16)

**Files:**
- Modify:
  - `ferb/sanitize/redact.py:58,184` (card pattern word boundaries, no match inside hex/alnum tokens);
  - `ferb/store/spans.py:49` (exempt structured id fields such as `source_refs` and node ids from text redaction);
  - `ferb/memory/consolidate.py:18-35` (strict-compatible schema: no open `additionalProperties` objects; model the
    slots/args/apps as arrays of `{name, value}`);
  - `ferb/policy/security_digest.py:12-13` (string `network`);
  - `ferb/daemon/main.py:1295-1309` (distill and consolidate in separate error scopes).
- Test: `tests/test_learning_alive.py` (new).

- [ ] Tests:
  - (a) 20,000 random sha256-style node ids pass through `Redactor` unchanged; a real card number in prose is still
    redacted;
  - (b) `capture_episode` with refs from a redacted span input succeeds (the 35-task failure shape: `unknown source`);
  - (c) the consolidator schema passes a strict-mode validator (every object has `additionalProperties: false` and all
    properties required), and a `Consolidator` round-trip with a fake runtime returns proposals;
  - (d) the security digest handles `network` as a string;
  - (e) a consolidator exception doesn't stop the distiller in the hourly loop.

  All fail first.
- [ ] Implement, validate, commit `fix(L16): ids never redacted; strict consolidator schema; hourly loop survives (F8, D9)`.

### Task 12: Typed text works safely; choice answers never become secrets (L17, L18)

**Files:**
- Modify:
  - `ferb/sense/watchers/text_input.py:68-69` (call `CGEventKeyboardGetUnicodeString` with a real max length) and its
    capture gates;
  - `ferb/daemon/main.py:2556-2558`;
  - `ferb/sanitize/redact.py:93-96`.
- Test: `tests/test_typed_text_capture.py` (new) and `tests/test_choice_answers_not_secret.py` (new).

**Behaviour:**
- **Capture.** Typed characters are captured per field (D7).
- **Never captured:**
  - secure text fields (`AXSecureTextField`);
  - while Secure Input is enabled (`IsSecureEventInputEnabled`);
  - in terminal apps. That's a where-boundary: the bundle ids of Terminal, iTerm2, Warp, Ghostty, kitty, Alacritty,
    WezTerm and Hyper, kept as one named constant with a comment citing L17.
- **No digit-shape rule** (§8.5).
- **Hidden secrets.** Only values entered through the sealed secret input are remembered as hidden secrets. An answer
  to a question with options never is.

- [ ] Tests:
  - (a) the adapter with a synthetic key event (PyObjC, as the reviewer reproduced: max 0 → `(0,'')`, max 4 →
    `(1,'a')`) feeds the typed character;
  - (b) the secure field / Secure Input / Terminal bundle → nothing recorded;
  - (c) after a 2FA-labelled choice question answered "1", the text "freed 1.5 GB in 11 files" passes through redaction
    unchanged;
  - (d) a value entered through the sealed input is still redacted in later text.

  All fail first.
- [ ] Implement, validate, commit `fix(L17,L18): typed text captured per field, never secrets; choice answers not secrets (D7, D28)`.

### Task 13: Waits after sleep, a free event loop, no dropped requests (L21, L22, L23)

**Files:**
- Modify:
  - `ferb/orchestrator/plan_executor.py:36-46` (time waits on the durable wall-clock timers in `ferb/clock/timers.py`);
  - `ferb/orchestrator/supervisor.py:298-318` and `ferb/orchestrator/plans.py:54-70` (inventory via
    `asyncio.to_thread`);
  - the Codex adapter's account-error path. Read 5b10afb first and add only what is missing.
- Test: `tests/test_waits_and_loop.py` (new).

**Behaviour:**
- **Waits.** A "wait until T" fires at wall-clock T even if the monotonic clock paused.
- **Inventory.** A home-sized inventory doesn't block other RPCs.
- **Account errors.** A Codex `account/read` failure gives the request a chat reply naming the failure. The request is
  never silently dropped.

- [ ] Tests:
  - a wait with a fake clock where monotonic stops and wall time jumps fires at the wall time;
  - while a slow inventory runs, an unrelated coroutine on the loop proceeds;
  - an adapter stub raising the account error yields a reply event for that `request_id`.

  All fail first.
- [ ] Implement, validate, commit `fix(L21,L22,L23): wall-clock waits, inventory off the loop, account errors reported`.

### Task 14: Questions never expire; profile question once; local services direct (§8.1, L3, L24)

**Files:**
- Modify:
  - `ferb/daemon/questions.py:10` (remove the 600 s expiry; an approval stays pending while its task waits; it closes
    when its task ends);
  - `ferb/daemon/persona.py:57-83` (create once; `status()` never recreates it; no banner);
  - `ferb/act/shell/sandbox.py:177-180` (direct connections to executable-pinned local ports, per the owner's Q1
    answer; record them).
- Test: `tests/test_questions_persistence.py` (new) and additions to the sandbox tests.

**Behaviour:**
- Pending questions persist until answered or until their task ends.
- The persona question gets one approval id across any number of `chat.status` calls.
- An approved local port is reachable directly from the shell sandbox once the listening executable matches its pin,
  checked when the command's profile is built. The connection is recorded in the `shell.network` journal with
  `local=true`.

- [ ] Tests:
  - an approval older than 600 s is still pending;
  - 100 `chat.status` calls produce one persona approval and no publish after the first;
  - a pinned local port appears in the generated profile only when the pin matches;
  - a non-matching executable is excluded.

  All fail first.
- [ ] Implement, validate, commit `fix(L24): questions persist; profile question once; pinned local ports direct (Q1)`.

### Task 15: UI chat flow (L6, L7, L8, check mark)

**Files:**
- Modify:
  - `ui/mac/FERBNotch/Sources/FERBNotch/DaemonConnection.swift:174-243`;
  - `Sources/NotchCore/ConversationArchive.swift`, `ConversationTranscript.swift`, `Conversation.swift`;
  - tests `ContinuousChatTests.swift`, `ReplyStreamingTests.swift:35-45`.
- Test: new `Tests/NotchCoreTests/ChatFlowTests.swift`.

**Behaviour** (against the event contract):
- **Turn display.** `ack` shows at once as a FERB line in the turn. `plan` shows as the next FERB line, and no longer
  goes to the activity feed only.
- **Live line.** `progress` updates one line under the plan with elapsed time since `started_at`. It is removed on
  `outcome`.
- **Late answers.** An `outcome`/`reply` whose `request_id` turn isn't the newest appends a reply entry at the bottom.
  The entry has a one-line quote of the question, and the original turn shows "answered below". History keeps both.
- **Check mark.** `verified=true` shows a small check mark.

- [ ] Tests:
  - an ack renders before any other event;
  - progress updates in place and clears on outcome;
  - A answered after B → a bottom reply quoting A, with A's turn marked;
  - the check mark shows on verified.

  All fail first.
- [ ] Implement, `swift test`, commit `feat(L6,L7,L8): UI shows ack, plan, live line, late answers as replies`.

### Task 16: UI questions, notices, and hygiene (L9, L20)

**Files:**
- Modify:
  - `ui/mac/FERBNotch/Sources/FERBNotch/NotchModel.swift:320-381` (questions inline, typing enabled; remove the paste
    dialog) and `:349` (no draft persistence; delete any stored draft at launch);
  - `ApprovalView.swift`;
  - `DaemonConnection.swift` (render `attention.notice` and `chat.status.notices` in chat with a "noticed" tag and
    quotes; send `attention.ack` for shown notices; acknowledge shown results so the CLI doesn't replay them; fix the
    `bus.dropped` topic name);
  - recovered tasks: never shown as owner-typed text.
- Test: new `Tests/NotchCoreTests/QuestionsAndNoticesTests.swift`; update `PastedRequestTests.swift`.

- [ ] Tests:
  - an open question leaves the composer enabled;
  - paste plus typing sends without a dialog;
  - drafts are never written to `UserDefaults`;
  - a notice renders in chat and is acked;
  - the `bus.dropped` topic is handled.

  All fail first.
- [ ] Implement, `swift test`, commit `fix(L9,L20): questions inline, notices in chat, no stored drafts`.

### Task 17: Extension auto-install at setup (L25)

**Files:**
- Modify:
  - `ferb/act/browser/profiles.py` (discover all installed Chromium-family browsers and profiles);
  - the setup and extension install code (`ferb/cli/admin.py`, `ferb/cli/__init__.py` `browsers extension`, setup
    steps);
  - native host registration;
  - config (`[browser] store_extension_id`, empty by default).
- Create: `ferb/act/browser/autoinstall.py`.
- Test: `tests/test_extension_autoinstall.py` (new).

**Research first (write it in the report).** Confirm from official docs, for Chrome, Brave, Edge, Chromium, Vivaldi,
Arc and Opera on macOS:
- the per-user External Extensions location and JSON (`external_update_url` =
  `https://clients2.google.com/service/update2/crx`);
- whether off-store `external_crx` is blocked on macOS;
- the native messaging host directories.

**Behaviour:**
- `ferb setup` and `ferb setup --update`, for each installed browser with an existing user-data directory:
  - register the native host with `allowed_origins` for both the pinned unpacked id and the store id;
  - when `store_extension_id` is set, write the External Extensions request;
  - report per profile: installed, enabled, paired (read Preferences/Secure Preferences only).
- Never write a browser's Preferences, never restart or open a browser.
- When the store id is empty, keep the Load-unpacked instructions and say the store listing is pending.
- `ferb browsers extension package` writes the store-ready zip.

- [ ] Tests (temp home with fake browser dirs for Chrome, Brave and Edge):
  - the request files and native host manifests are written for each;
  - the Preferences files are byte-identical before and after;
  - an empty store id writes no request and prints the manual steps;
  - status per profile is read correctly from fixture Preferences.

  All fail first.
- [ ] Implement, validate, commit `feat(L25): setup requests the extension in every Chromium browser and profile`.

### Task 20: Hearing system sound during tasks; media control (L26)

**Files:**
- Create:
  - `ferb/act/mac/audio.py` (ScreenCaptureKit app-audio capture; PyObjC, following `ferb/act/mac/screenshot.py`'s use
    of ScreenCaptureKit);
  - `ferb/sense/transcribe.py` (Apple on-device speech: `SFSpeechRecognizer` with `requiresOnDeviceRecognition`, on
    the captured file).
- Modify:
  - the action registry and worker tools (`audio.listen`);
  - the browser tools and extension `content.js`/`background.js` (`browser.media`, plus media state in snapshots);
  - recording tools (`with_audio`, transcript in the result);
  - setup/doctor (speech permission);
  - `ferb/orchestrator/prompts/worker.md` (how to hear, and to confirm playback);
  - `docs/RPC.md`, if the extension protocol changes. Keep the extension version 0.15.3, unreleased; if the protocol
    changes, bump `PROTOCOL_VERSION` and the JS protocol together per CLAUDE.md.
- Test: `tests/test_audio_listen.py`, `tests/test_browser_media.py` (new), and a `-m real` live test the owner runs.

**Interfaces:**
- `audio.listen {app? | tab?, seconds}` → `{transcript, segments:[{start, end, text}], audio_path, app, seconds,
  on_device: true}`.
- `browser.media {tab, action: play|pause|mute|unmute|seek, to?}` → `{state}`.
- Snapshot `media: [{index, playing, current_time, duration, muted}]`.

**Behaviour:**
- Spec §4.9 exactly.
- The model chooses the duration. There is no FERB cap, and none may be introduced.
- Listening stops when the task ends or is stopped.
- The transcript goes through the Redactor and carries the app content's origin label.
- On-device recognition unavailable → a clear error, never a cloud call.
- Capture excludes FERB's own audio.

- [ ] Tests first, using fakes for ScreenCaptureKit and Speech at the adapter boundary:
  - `audio.listen` on a fake app stream returns a redacted transcript with segments and records a span (app, seconds);
  - a secret spoken in the audio (a card number in the fake transcript) is redacted;
  - on-device unavailable → error, and no network call;
  - task stop ends a listen in progress;
  - `browser.media play` on a page with a paused `<video>` → `playing: true` in the next snapshot;
  - `unmute` sets `muted: false`;
  - `browser.record with_audio` attaches a transcript;
  - the worker prompt tells the worker to confirm playback before recording or listening.

  All fail first.
- [ ] Implement, validate (full suite, mypy, diff-check), commit `feat(L26): FERB hears app sound during tasks, on-device transcripts; media control`.

### Task 21: Packaged FERB.app and DMG, ad-hoc signed now and signed later (L27)

**Research first (write it in the report, with sources)**, then implement the best-supported approach:
- **Embedding the daemon:** how to embed the daemon's Python into a macOS app bundle so it runs from any location.
  Candidates:
  - a uv-managed standalone CPython plus the `ferb` package and dependencies installed into the bundle;
  - py2app;
  - PyInstaller.

  Check: relocatability; PyObjC; MLX (if the embeddings dependency is optional, say so); code-signing every nested
  binary.
- **Background daemon:** how to run it from the bundle. `SMAppService` login item or agent: does it work for an
  ad-hoc-signed app? If not, use the existing `~/Library/LaunchAgents` approach that `ferb setup` already uses,
  pointed into the bundle.
- **Helpers:** where the native-messaging host and the secret helper live in the bundle, and how their requirements
  and paths change. The secret helper's `codesign -R` requirement must match the ad-hoc signature, and later the
  Developer ID one.

**Files:**
- Create:
  - `scripts/build-app.sh` (assemble and sign the bundle);
  - `scripts/build-dmg.sh` (DMG with the app plus an Applications link);
  - a packaging smoke test in `tests/test_packaging.py`, which checks bundle structure and Info.plist keys and
    builds into a temp dir when tools are available, otherwise skips with a reason.
- Modify:
  - `ui/mac/FERBNotch` (app bundle id/name "FERB", Info.plist usage descriptions, first-launch setup flow);
  - `ferb/device/install.py` and `ferb/cli/setup.py` (bundle-aware paths);
  - README install section ("Open Anyway" steps; source install kept).

**Constraints:**
- Don't edit `macapp/FERB` sources while Task 20 is changing them. Include the voice/transcribe app as a nested helper
  app built from `macapp/FERB` for now, and merge it into the main bundle after Task 20 integrates (note it as a
  follow-up in the report).
- Never touch the owner's installed `~/.ferb`, the running daemon or browsers, and never run first-launch setup against
  the real home. Build into the session scratchpad.

**Signing step:**
- `SIGN_IDENTITY` defaults to ad-hoc (`-`).
- When it is set to a Developer ID, the script also runs `notarytool` and `stapler`, with credentials taken from the
  environment and never written anywhere.

- [ ] Research, then tests first (structure/Info.plist/smoke), then implement. Validate: full suite, mypy core,
  diff-check, `swift test` in `ui/mac/FERBNotch`, and one real local build of the app and DMG into the scratchpad
  (report size and what launched). Commit `feat(L27): packaged FERB.app and DMG, ad-hoc signed; Developer ID ready`.

### Task 18: Launch campaign path through the gateway (D30, L22)

**Files:**
- Test: `tests/test_standing_permission_campaign.py` (new).
- Modify only where a test exposes a defect (cite it in the report).

**Behaviour to prove offline:**
- A standing permission stated in the acknowledgement ("post the launch story to X daily until Oct 21") lets posts
  inside its scope run with no confirmation. They are logged and appear in the send digest.
- A post to a different site, or a payment, still asks.
- Durable waits between posts fire on wall-clock time (Task 13).
- After a daemon restart the plan resumes.

- [ ] Write the tests, see which fail, fix the defects, validate, commit
  `test(D30): launch campaign under standing permission through the gateway`.

### Task 19: Docs and registry

**Files:**
- `docs/0.27_PLAN.md`: dated 2026-10-13 clarifications for Q3, D6/D26, D29, P11/Q4, TQ3, D24/rule 9.
- `docs/0.27_DECISIONS.toml`: L1–L25 entries, each with milestone "L" and its tests.
- `CLAUDE.md`: rules 1, 8, 9 as in spec §5.
- `AGENTS.md`: "Where things stood"; keep the existing uncommitted change.
- `ARCHITECTURE.md`, `README.md` (extension setup, UI in `ui/`), `CHANGELOG.md`.
- `docs/OWNER_ACCEPTANCE.md`: the live checks from spec §7.

- [ ] Update; `uv run pytest -q tests/test_decision_coverage.py`; full suite; commit
  `docs(L1-L25): record launch-week decisions, checks and setup`.
