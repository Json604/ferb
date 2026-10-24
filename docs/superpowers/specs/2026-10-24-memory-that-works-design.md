# Memory and learning that work — design (2026-10-24)

Status: the owner approved the direction on 2026-10-24 and answered its three questions (below). This written design awaits
the owner's review before an implementation plan. Decision id: L36. Built before the public launch (the owner's
"BEFORE LAUNCH", 2026-10-24), after the 2026-10-24 fix batch and FERB's cursor (L33). Proactivity is a separate design
that builds on this one.

## Why

On 2026-10-24 the owner told FERB "the username is Kartikey Attri" while correcting a running visa task. FERB saved
nothing. Hours later a plan step of a new visa task typed the owner's email as the username, twice. Asked "how good
is our memory", a read-only audit of the live database and code (scratchpad `memory-audit.md`, HEAD f747d37) found
that memory works for the planner and almost nowhere else:

- **Capture.** The owner's durable statements over 14 days: 8 messages were saved, 9 only proposed, 14 missed.
  - Code dropped everything said in a live correction. The steer path returned before the remember loop
    (supervisor.py:1194 vs 1287); this is fixed by the 2026-10-24 batch, commit 1c22dcb.
  - On 10 of the misses, the planner left `remember` empty.
  - The nightly distill makes `inferred` proposals that nobody is asked about. 27 are waiting, 0 questions have ever
    been asked, and 0 have been confirmed.
- **Recall.**
  - Every planner turn gets about 207 unranked memory lines (43 KB). 102 of them are bare task ids, and all of them
    are protected, so the conversation shrinks as memory grows.
  - Workers get none: the daemon's `recall` is a stub (main.py:1336).
  - Plan steps blank memory (supervisor.py:2969-2970) and lose the parent task's goal (supervisor.py:2235).
  - `memory.read` is a literal substring match: 14 of 20 pulls came back empty. `memory.learned` returns keys only.
- **Logins.**
  - Secret handles are free-text names bound to a place. There is no username field, no listing by site, and
    duplicates build up.
  - A resumed task couldn't see the handles saved that morning and re-asked for two of them.
- **Redaction.** Sealed values are matched as raw substrings everywhere (redact.py:905-908, 789-793). One sealed
  answer is an ordinary word, so that word reads `[redacted]` inside FERB's notes about the owner. The visa task's
  episode was refused twice, and on 10-14 48 captures failed because a short sealed value matched inside hex ids.
- **Stale facts.**
  - They never retire: the 10-11 wedding flight, "DevOps left", the RSNA competition (now over).
  - A later "book anywhere" reversal was dropped, so memory item 54 and schedule sc-eec2a61cbf still say "Chennai,
    notify first".
- **Learning.**
  - 0 procedures. All 5 consolidations were refused: actions were compared by exact arguments (tab, element and
    process ids change every run; `~/` paths vs expanded paths), and app versions had to be equal across examples
    (Brave 151 vs 155).
  - Only 5 of 68 intents have 3 verified runs, and 2 of those are test scenarios.
  - Nothing learned has been used: `learning_receipts` is 0.

## Owner decisions (2026-10-24)
- Before launch.
- **Guesses:** "Ask right away". Something FERB only inferred, as opposed to the owner's own words, becomes a question
  to the owner as soon as FERB makes it.
- **Memory size:** "Planner 60, worker 20". Each planner turn gets the 60 most relevant items; each worker and plan step
  gets the 20 most relevant, ranked.
- **Usernames:** "Yes, plain". A site's login username is ordinary memory that models can see and type. Passwords and
  security answers stay sealed in the Keychain.

## Rulings made in this design (the owner can correct them)
- **The backlog.** The 27 proposals already waiting, the oldest from 2026-10-01, are not turned into 27 questions at
  once. They appear once as a single "Review N older guesses" list (Keep / Edit / Drop each) in the notch, and later in
  the FERB window (L34 Phase E). New guesses are asked right away.
- **Identity.** The 8 owner-stated preferences about the owner stay in every role's system prompt (the identity
  layer). They don't count toward the 60/20.

## Design

### 1. Capture on every path (done)
Commit 1c22dcb applies the planner's remember, forget, reminder, schedule, notification and conversation entries once
on every turn path, corrections and stops included. f747d37 tells the planner to remember a login name in the same
turn.

### 2. A safety net that saves the owner's words
- Distill (`ferb/memory/distill.py`, prompt `distill.md`) may cite the owner's exact words for an item: a `quote` field
  holding a chat message's text and that message's journal id.
- Code runs the existing chat-capture check (`contains_words` and `owner_sentences`, items.py:119-130) against that
  journal entry's typed text, never pasted parts.
  - When the check passes, the item is saved as `owner_stated`, active, with the journal entry as its source, exactly
    as if the planner had captured it in that turn.
  - When it fails, or there is no quote, the item is an `inferred` proposal.
- Each new inferred proposal (from distill, or from chat capture as today at main.py:2180) becomes a question at once.
  It uses the existing confirm question (`_ask_owner("confirm", …)`): "Keep this? “…”", with Keep / Edit / Drop.
  - The question never expires while unanswered (rule 9).
  - Keep makes the item active, owner-confirmed. Edit saves the owner's text as owner_stated. Drop forgets it.
- An item that updates an existing key (same subject and key) supersedes the old version through the existing
  versioning (`supersedes`, `valid_to`).
- The agent decides what is durable; no keyword list decides it (rule 6).

### 3. One ranked search
- **One retrieval function.** `Learning.search` becomes the one function used by the planner turn, attention, worker
  and plan-step push, `memory.read` and `memory.learned`.
- **Content-only index.** It matches over content: fact and entity text, episode goal and outcome, procedure intent.
  JSON field names and scaffolding are left out of the index.
- **Ranking.** Matching uses the existing document-frequency filter (items.py:269-271): a term found in more than half
  of the items carries no weight. Ranking is by bm25 with a small recency boost; when the owner has adopted an
  embedding model, its similarity is used as well.
- **Budgets.** The owner's 60/20, applied after ranking.
  - Owner-stated items rank before inferred-confirmed items at equal relevance.
  - Blocks are no longer all `protected`. Only identity and items the turn names by id stay protected.
- **Episodes** show their goal, outcome and date instead of a bare task id. Episodes without a stored goal are left out.
- **The tools.**
  - `memory.read` keeps its arguments, but each query is ranked through the same function (no substring match).
  - `memory.learned` returns content, not only keys (still with ids).

### 4. Every worker and plan step gets memory and its parent goal
- `recall` in the daemon (main.py:1336) is wired to the one search. The query is built from the owner's request text,
  the task goal, and (for a plan step) the step goal. Budget: 20.
- Plan steps stop blanking memory (supervisor.py:2969-2970).
- A plan step's context gains "Parent task goal: …", the full goal the planner wrote, above its step goal
  (supervisor.py:2235). Nothing else about step scoping changes.
- What is pushed is recorded in `task_memory_refs` as today, so forget still reaches it.

### 5. One login record per site
- **The record.** A login record per HTTPS origin (or per app for app-bound secrets) holds:
  - the username, as plain text;
  - named sealed entries: the password, and each security question's answer under that question's text.
- **Where it lives.** The record lives in the secrets store's handle file (`owner_secrets.py`), extended from
  "handle → target" to "origin → {username, entries: {name → handle}}". Values stay in the Keychain under service
  `ferb.owner-secrets` as today.
- **Saving.** Saving an entry whose name already exists for that origin replaces it, so no more duplicates. The worker
  chooses the entry's name; the place stays code-derived (D12).
- **The username.**
  - It is set when the owner gives it. A planner `remember` entry gains an optional `login_origin`, meaning "this
    content is the username for this site". Code writes it to that origin's record only when the site appears in the
    owner's own typed words (this message, or the task's recorded owner words), and only from an owner_stated item.
    Built tighter than first written (2026-10-25 security reviews): a site only the planner or outside content named
    never gets the owner's username; sites link exactly ("www." aside), never by subdomain; a username shown through a
    link names its source site, and only when the task names one site.
  - It is shown in the hidden-input question ("Password for Kartikey Attri at usvisascheduling.com").
  - It reaches models through memory (owner decision: plain).
- **A new tool, `secrets.list(origin)`** (observation), returns the username and the entry names for one origin. It
  never returns values. A worker on a login page calls it to see what's already saved before asking the owner.
- **Migration.** Existing handles are grouped by their bound origin. The old app-bound visa password, which
  `refuse_whole_browser` now refuses, is listed as unusable with "ask again".

### 6. Out-of-date facts retire
- **Supersession.** When the owner's later words update a remembered item (the planner reuses its key, as its prompt
  already says), the new version supersedes the old one. This now works on every path (1c22dcb).
- **Dates.** A fact whose owner words carry a date gets `valid_to` at the end of that date (the planner's `remember`
  entry gains an optional `until`, which code checks against the owner's typed date words). After that date the item
  leaves search, as `valid_to` already makes it.
- **Distill.** Distill may propose retiring an item, citing the owner's words. That follows the same quote check as
  section 2; a retire without words becomes a question.

### 7. Redaction matches sealed values safely
- A sealed value is replaced only where it appears as a whole token (word boundaries, or field boundaries for
  structured values). It is never replaced inside FERB's own ids (ids keep their form; a collision there is
  impossible to mistake for the secret).
- The redaction itself stays: a sealed value never reaches a model, log or trace (rule 14).
- **A sealed value that is a single ordinary word.**
  - At save time FERB says once: "This answer is an ordinary word, so FERB hides it wherever it appears."
  - Code decides "ordinary" from the macOS system dictionary, no hand-made list.

### 8. Learning unblocked
- **Comparing procedure steps with examples** (learning.py:888-904):
  - Per-run handles are normalized before comparison: tab, element, process, window and span ids, temporary paths.
  - `~/` and home-relative paths are expanded on both sides.
  - Arguments marked as slots must match the example's slot value; the rest must match after normalization. The
    "reproduces a cited action" rule stays.
- **App versions** across examples become an observed range (min–max per app) instead of one required version. A
  later run on a version outside the range runs the procedure with "new version: verify each step".
- **Grouping.** The intent labeler (`label:` prompt) is told to name the task's purpose so repeats group together ("keep
  a background tab's media playing", not per-site variants). The owner's 3-verified-runs counter stays.
- **Use is recorded.** When a worker's context includes a learned procedure and the worker follows it, the receipt is
  written (`learning_receipts`). `ferb stats` shows procedures learned, used and refused.

## Not in this design
- Proactivity: offering and taking up work (separate design; it uses this memory).
- The FERB window's Memory page (L34 Phase E). It uses `memory.*` RPCs, which keep working.
- A new embedding model. The existing optional local model is used when adopted.

## Testing
- **The username case.** An owner steer "the username is X" during a live task saves a fact and a login-record
  username. A new task's plan step that logs in to that site receives the fact in its memory block, and
  `secrets.list` returns the username.
- **Capture.** Distill with a valid quote saves `owner_stated`. With a missing or mismatching quote, or with pasted
  text, it saves `inferred`, and a question is asked at once. The backlog list appears once.
- **Recall.** On the audit's real queries, the scratch-DB reproduction becomes a fixture: the visa username ranks
  within the worker's 20 and the planner's 60. Episodes show goal and outcome. `memory.read` with a multi-word phrase
  finds the item. The budgets hold.
- **Plan steps.** The context contains the parent goal and the memory block.
- **Logins.** Saving the same entry name twice leaves one entry. `secrets.list` never returns values. Migration
  groups the existing handles.
- **Redaction.** A common-word sealed value is redacted as a word, not inside ids or longer words. The save-time line
  appears once.
- **Retirement.** A dated fact leaves search after its date; a superseding steer replaces item 54's Chennai preference.
- **Learning.** The audit's 5 refused consolidations, as fixtures, pass when the steps reproduce the actions modulo
  handles. A real mismatch is still refused. Version ranges work. A receipt is written on use.
- **Live, owner-observed.** Tell FERB a fact while it works, start an unrelated multi-step task that needs it, and
  check the step used it. Log in to a site twice; the second login asks for nothing already saved.
- L36 in `docs/0.27_DECISIONS.toml`; ARCHITECTURE.md (memory and secrets sections), README (memory), CHANGELOG.md.
