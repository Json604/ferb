# Plan: one context design for every AI (ORCH-719)

The owner's goal: "how we can do it efficiently - NO MATTER WHAT AI WE ARE USING". Agreed in discussion on 2026-11-25
after a measured audit and two independent reviews (kept untracked in `.local/`: context-audit.md, context-strategy.md,
context-design-draft.md, context-design-review.md). Costs are judged as rates for light and heavy users, never by the
owner's test traffic. Numbers marked (owner) are his decisions; everything else is an agent mechanism.

## What the records showed (2026-11-17..25)
139.8M input tokens, 90% cached reads. Workers 86%, checker 9%, planner 4%. Old page snapshots resent on every later
step are ~45% of worker input; full tool descriptions (~28k tokens, 76 tools; 6-8 used a run) are on every step; the
checker rereads the worker's whole tool log; the planner reread the whole chat (the fold's token triggers were never
reached). Clearing old results existed only for Claude Code (rewritten in FERB's relay, RUN-470) and FERB's API loop;
Codex had none (RUN-472). Anthropic refuses client-edited histories on its newest models for accounts made from
2026-08-31 and on every account for Fable 5.2.

## Principle
FERB fixes waste where it has control on every runtime: what its tools return, which tools it offers, what it puts in
the starting instructions, when a run starts and ends, and the planner's and checker's whole input. A runtime's own
compaction stays only as a safety net. One path for every AI.

## The design
1. **Measure and guard.** `ferb stats` shows per role and runtime the tokens per call and step, the cached share and
   the fixed part; FERB's Claude relay counts usage; a hash per context block names what broke a cached prefix. Tests
   fail when a role's fixed part passes its ceiling, the planner prefix stops being byte-stable, or a long chat has no
   summary. Usage is recorded even when a run fails. A detector notices Anthropic's refusal of an edited history.
2. **Snapshots: the record stays whole, the view is what changed.** FERB records every look in full (the checker and
   task.evidence read the record). The worker gets the difference from the last look at the same page plus a
   reference; the whole page is sent on a first look, a new page, when more than half changed, every 6th look in a
   row, after any compaction or fresh run, and on request. A store of earlier looks serves references.
3. **Tools: a fixed core plus find/use.** Each worker type has a fixed core (at most 25 full descriptions, owner) and
   the checker at most 10 (owner); every other tool is always listed by name and one-line purpose; `tools.find` gives a
   tool's full description and a direct call to any listed tool works (FERB answers with its description when needed).
   The list never changes mid-run (caching and Anthropic's thinking binding). The core per worker type comes from the
   records. A weekly guard flags a run that said it couldn't when a fitting tool existed.
4. **Fresh run.** When a worker run's conversation passes 200k tokens, or 70% of a smaller model's window (owner), FERB
   ends the run and starts a new run of the same task with a hand-off note built by code from FERB's records (goal;
   what was done from the ledger; what is left; key facts; references to the latest page, app and file), at most
   1,500 tokens. Never while a question, confirmation or sealed fill is open; it doesn't count as an attempt; at least
   10 tool calls between fresh runs, an alarm past 5 in one attempt. The runtime's own compaction is the safety net
   at 245k (owner), or 60% of a smaller window. RUN-470's request rewriting is retired (off by default once 2 lands).
5. **Checker: the middle path.** The newest look at each page, app window and file in full, plus everything the
   answer cites; the rest as a searchable index with claim-to-evidence pointers. Tested offline on the recorded checks
   before any replay. (Supersedes the parked slim checker's index-only approach.)
6. **Planner history: a bounded calendar.** Always sent, whatever FERB's age: the last 20 exchanges word for word
   (owner), with a hard stop at 40 exchanges or 20k tokens that needs no model call (owner); a dated topic index for
   the last 14 days, at most 6 lines a day (owner), each line written once from the original messages and never
   rewritten; one line per older month (from that month's lines) and per older year, at most 40 words each; the newest
   15 open items from live records plus a count (owner), placed after the cached part. The index is a pointer, not a
   record (PRM-373); the full chat stays stored and searchable; "forget" removes its index lines. A fold after 20 more
   exchanges plus the token triggers: about one extra model call per 20 messages.
7. **Profile.** Only confirmed lines are always sent, at most 1,500 tokens (owner); the rest are lookups. Workers and
   checkers get standing rules and what their task needs, not the blanket profile. A profile change sits after the
   cached prefix.
8. **Planner trims.** One browser-profile list; no per-entry ages line; the "latest result" block only when not already
   in the conversation; find and fix the Claude planner turns that send the whole prompt 2-3 times.
9. **Worker fixed prefix.** Live facts after the fixed part; "previous attempts" bounded; no blanket profile.
10. **Bugs.** The profile pass's due-check (identical input hourly); usage on failed runs. (The 1 MB reply limit is
    fixed: RUN-473.)

## Build order (each a task)
1. Measurement, guards and the refusal detector (T-1330).
2. Bugs and free wins: profile due-check, the hard stop on raw chat with the exchange-based fold, checker block order
   for caching, the repeated Claude planner requests, the planner trims (T-1331).
3. Snapshot record/view split, the store and the whole-page rules, with an offline replay first (T-1332).
4. RUN-470 off by default; RUN-472 superseded (T-1333).
5. Fixed tool core with find/use (T-1334).
6. Fresh run, the code-built note first (T-1335).
7. Checker middle path, tested on the recorded checks (T-1336).
8. The full day/month/year index and the profile core (T-1337).
Expected (a model over the records, to be measured): workers -40 to -50% tokens; planner 27-32k per message at any
age (44-80k today).
