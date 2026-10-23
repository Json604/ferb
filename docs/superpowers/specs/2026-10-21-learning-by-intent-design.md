# Learning by intent — design (2026-10-21)

Status: direction approved by the owner on 2026-10-21 ("i go with your recommended run. BUT just have some connection
to the tools used for the previous learned things so that model gets faster with steps"); this written design awaits
the owner's review before an implementation plan. Decision id: L29.

## Why

FERB is meant to get better at tasks it repeats. As built, it never has:
- Every learning call failed (226 of 226 `learning_jobs` from 2026-10-11 to 2026-10-20, measured on a copy of the
  owner's database): each card went with its whole lineage list, a median 1.07M characters, over Codex's per-input cap
  of 1,048,576 characters. No procedure or lesson was ever made. Fixed on `astra_resume` (fix waves 3 and 5: cards go as
  `card_digest`, median 10.7k characters; the cap is checked before sending; failures carry Codex's words).
- The design gap remains: cards are grouped by `card_shape` — the exact set of tools plus the exact set of apps
  (`ferb/memory/learning.py:45`, `ferb/memory/consolidate.py:153`) — and the model sees one group per call. 85 cards fell
  into 81 groups; "find me the cheapest option for egg curry with some breads" ran 3 times and landed in 3 groups, so
  the model never saw the same task three times together and could never propose a procedure for it.

## What the owner gets

After FERB has done the same kind of task three times, it writes down how it did it (a procedure: the tool steps with
blanks for what changes, plus lessons from what failed). The next time, the planner picks that procedure and the worker
starts from its exact tool steps and the latest runs' steps instead of exploring from scratch, checking each step's
expected result as it goes.

## Design

### 1. Each finished task gets an intent label (new, model-assigned)
- When an episode card is written (`Learning.prepare_episode`, `learning.py:259`, in the card reader), an idle-time
  learning call labels it: the model gets the card's digest and the current intent list (each intent's id, short name,
  one-line description and slot names) and returns either an existing intent id or a new intent (name, description,
  typed slots), plus this run's slot values. The model decides sameness (R2: no keyword list; a shared word is not
  sameness — the existing rule in `consolidate.md`).
- Code checks the answer: the id exists or the new intent is well formed; for an existing intent, the slot names match
  its declared slots and the values match their JSON types. A refused or failed label keeps its job error and is tried
  again when the card changes or on `ferb memory consolidate` (as consolidation jobs do today).
- Stored as the card's own label: a learning node of kind `episode_intent`, keyed by the card's node id, whose only
  source is the card. It carries the intent's id, name, description and slot types, and this run's slot values. The
  intent list is the distinct intents of the current cards' labels, so forgetting a card's source removes only that
  card's label (existing lineage and forget rules); an intent lives while any current card carries it. (Not the
  existing `episode_annotation`, which consolidation also writes for the same key: two writers would supersede each
  other. Not one node per intent citing all its cards: forgetting any one card would delete the intent and everything
  built on it.)
- The intent list is fitted to the learning runtime's window with the existing context machinery (`assemble`, L28);
  no count cap.
- Backfill: the existing cards are labelled on the first idle pass, one call each.

### 2. Cards are grouped by intent, not by tools
- `Consolidator._groups` groups by the card's intent label. Unlabelled cards wait for their label.
- `card_shape` stays as a fact shown with each run (the tools and apps it used), not as the grouping key.

### 3. Learning starts after three runs of one intent (the owner's "counter 3")
- An intent is consolidated when it has at least three verified runs not yet consolidated together, and again whenever a
  new run of an intent that already has a procedure finishes (verified or failed), so the procedure and its lessons are
  updated with what worked and what didn't.
- One call per intent: all its runs' digests (verified and failed, fitted to the window, newest first when they don't all
  fit), its current procedure and lessons, and the owner's refusals. Output as today: procedures, lessons, identity
  proposals (`PROPOSAL_SCHEMA`).

### 4. Procedure checks keep evidence, allow runs that differ slightly
- Kept from `propose_procedure` (`learning.py:427`): only verified examples; typed slots with values in every example;
  each step cites real successful actions of the same tool; app-version preconditions as captured; one task counts once;
  a step built on outside content quarantines the procedure.
- Changed: a step must reproduce a cited action in every example where it appears, not in every example. A step that
  some examples didn't need is marked optional (its examples are listed). Today one extra scroll in one run blocks the
  whole procedure.

### 5. The next run starts from the learned steps (the owner's "connection to the tools")
- The planner is shown the intents that have procedures (name, description, slots, state), fitted to its window, and may
  choose `procedure_id` with this request's slot values. This replaces matching procedures by the request's words
  (`Learning.candidates`, `learning.py:702`) with matching by meaning.
- The worker gets, with the procedure (`procedure_text`, `learning.py:706`):
  - its steps as a ready tool plan: each step's tool, its arguments with this run's slot values filled in, and the
    expected result to check;
  - the exact tool calls of the latest successful runs of that intent (tool, arguments, what came back), newest first,
    fitted to the window — so it reuses what worked instead of searching for the right tools again;
  - its lessons.
- A learned procedure is the fast path (as today: follow the steps, check every expected result, fall back to full
  reasoning on any mismatch); a candidate is evidence only. A procedure never grants permission (unchanged).

### 6. Promotion is unchanged
`Learning._refresh` (`learning.py:523`): learned at three verified successes with two or more slot variants and matching
app versions; two failures in a row make it suspect. The examples a procedure is built from count as receipts
(`propose_procedure`), so three runs with different values (egg curry, paneer) give a learned procedure at once; three
identical runs give a candidate until a run with a different value succeeds.

## Not changed here
- Daily distillation and roll-ups (`distill.py`). A day over the per-input cap still waits (the open "distill day over
  the cap" question).
- Facts and identity proposals (unchanged rules).

## Testing
- Offline (fake runtime): repeated runs of one task get one label; a different task gets another; grouping follows
  labels; consolidation waits for three verified runs and re-runs on a new run of a procedure's intent; optional steps
  validate and a step that reproduces nothing is refused; the planner sees intents with procedures; the worker's context
  carries the filled steps and the latest runs' tool calls, fitted to small windows; forget removes labels with cards;
  promotion as today.
- Live (owner): the same kind of task three times with different values (e.g. "find the cheapest <dish> with breads"),
  then `ferb memory consolidate`; `ferb memory search <dish word> --kind procedure` shows a procedure; a fourth run's
  `ferb trace TASK --full` shows the procedure and steps in the worker's input; compare its tool calls and time with the
  first three (`ferb baseline`).

## Open for the owner
- None blocking. The "distill day over the cap" question stays open separately.

## Amendment 2026-10-23: browser procedures without a hold
Owner, 2026-10-23: "i dont want that annoying rec again, cant we just leave browser thigns as it is … make it just
bare minimum so that our loop also does not break" (plan `docs/superpowers/plans/2026-10-23-perf-and-browser-holds.md`,
Task 2). It amends §4's "a step built on outside content quarantines the procedure" in the card reader only
(`learning.outside_targets`); the gateway is unchanged.
- A step's site never makes it outside. Under L2 FERB reaches any site without a question, and the page FERB read names
  its own address, so every typing or clicking step on a site FERB had read counted as outside.
- An action with no recorded target provenance (the reversible kinds: opening a site, app or tab, focusing, the Trash,
  timers, recovery points, stopping a command) is FERB's own choice, not outside.
- Unchanged: a recipient, account, contact, path or amount found only in outside content makes the step outside and
  holds the procedure until the owner confirms; the owner's words and answers are the owner's, and "unknown" is not
  outside; the confirm carry-over across revisions is the same rule.
- The card composer becomes `code:evidence-card/4`, so every card is read again once; labels carry forward without a
  model call. The earlier option of recording provenance for the reversible kinds is moot.
