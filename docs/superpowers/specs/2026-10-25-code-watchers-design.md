# Code watchers: long-running watches without AI calls — design (2026-10-25)

Status: PARKED by the owner on 2026-10-25 ("lets drop this. not doing this now. lets stay on track"). Not
scheduled; kept for a later decision. Decision id reserved: L38.

## Why

The owner (2026-10-25): "if some task is frequent and is gonna be done for a long time, then that might take too much
llm calls, will ferb make a code solution for that?" They shared a hand-written DevTools script that reloads a job page
every 20–40 s, finds "Claim / Start task / Accept task" buttons (not "Claimed / View task"), compares them with the last
set, and alerts on new ones.

Today's loops (L31) already watch without the AI between rounds, but only partly:
- **One read-only fact.** A probe re-reads one fact (one field of one observation tool; equals, not_equals or changed)
  every 10 s, with no AI. The owner's Instagram loop made 64 reads in 11 minutes with no AI call.
- **No refresh.** A probe can't reload a page: it allows only read tools (`probe_safe`).
- **No set compare.** It can't compare a set of items ("a new one appeared"), so a reload-and-scan watch would cost one
  AI round per reload.

## Owner decisions (2026-10-25)
- **What a cycle may do with no AI:** "your recommended". That means reloading the page, reading it, and repeating
  clicks set up once that only switch the view (a tab, a filter), matched by their exact accessible name.
  - Anything else (claiming, booking, sending) wakes an AI round that does what the owner asked, with the usual
    confirmations.
  - It's a general capability, not one site's feature ("is it a hard coded feature you are adding??? will this only
    cater the example i shared").
- **Poll speed:** "30s floor — it searches online if it can find the particular rate limit for that service. convey to
  the user and tell whats better".
  - A watcher never runs faster than every 30 seconds.
  - When it's created, FERB looks up the service's documented rate limits or terms. The plan line says what it found and
    which interval it chose, and why. The owner corrects by replying.

## Design

### 1. A watcher is a recipe of FERB's own actions
- **Where the recipe lives.** A loop step may carry a `watch` recipe instead of, or in addition to, a single-field
  probe.
- **Who writes it.** The loop's first round writes the recipe, which is data, through a new worker field `watch` (as
  `probe` is today).
- **Who runs it.** Code runs it on every cycle until the loop stops.
- **The steps, in order** (a fixed vocabulary; no page JavaScript):
  1. `refresh`: reload the bound tab, or navigate it to its recorded URL. Optional.
  2. `view`: zero or more clicks, each found by exact accessible name and role in a fresh snapshot (e.g. role tab,
     name "Available tasks"). A click runs only when the element isn't already selected. A `view` element that's
     missing or changed is an anomaly (section 4), never a guess.
  3. `settle`: wait until the page has loaded and a required element is present, or up to N seconds (≤ 30).
  4. `read`: a snapshot through the gateway. The browser snapshot (or another observation tool) runs under the task's
     identity and grant.
  5. `pick`: from the snapshot, extract either
     - **items**: elements whose role is in a set and whose accessible name matches an include pattern and not an
       exclude pattern, each keyed by a stable id (href, data id, or a hash of its card text, as the owner's script
       does); or
     - **a value**: one element's text or a field (a price, a status, a count, a date list).

     The patterns are the AI's reading of the owner's words, run by code as data. Code never judges intent (rule 6),
     exactly as a probe field is matched today.
  6. `compare` with the last cycle: `new_items` (an id not seen before), `gone_items`, `value_changed`,
     `value_crossed` (above or below a number), or `value_equals`.
  7. `then`, on a trigger:
     - `notify`: alert the owner, with code-written text listing the new items' titles as page data, labelled with
       their site;
     - `offer`: an L37 offer such as "Claim it?";
     - `round`: wake an AI round to do what the owner asked ("claim any new task as soon as it appears").
- **All steps run through the gateway:** policy, the destructive floor, the sink rule, site recording (L2). A `view`
  click goes through the normal click path, so any confirmation it would need pauses the watcher and asks, never
  skips. `refresh` and `view` count as acting calls for the round-check rule (owner, 2026-10-25: rounds that act are
  checked). The watcher's own cycles are code runs, not rounds.

### 2. Pacing
- **The interval** is a random range chosen by the first round from the owner's words, e.g. "every 30 seconds" → 30–45
  s; "as fast as is safe" → the researched safe interval. It has a hard code floor of 30 s (owner).
- **Rate-limit research at creation.** The first round searches for the service's documented rate limits or
  automated-access terms (web search, through the gateway). It reports what it found in the plan line, with sources,
  and chooses an interval at or above both the floor and the documented limit. Example: "Handshake publishes no limit;
  similar sites flag faster than about one request a minute, so I'll check every 45–75 s. Reply to change it."
  - If the owner asks for faster than the floor, code refuses and the plan line says why.
  - If the owner asks for faster than the found limit but at or above the floor, it's allowed, and the plan line warns.
- **Back-off:** the existing pacer handles repeated anomalies (L31's back-off).

### 3. Cost
- A cycle makes no AI call. AI is used only to create the recipe (round 1), on a `round` trigger, and on anomalies.
- **Recording:**
  - A cycle records one compact `watch.cycle` row: time, outcome (no change / triggered / anomaly) and counts, but not
    the page text.
  - The site visit is recorded (L2).
  - Full snapshots aren't stored per cycle, so the database doesn't grow at reload speed. A triggered cycle stores its
    read once as evidence, for the alert or the round.

### 4. Anomalies wake the AI, never a guess
An AI round (paced by back-off) runs when:
- the tab closed or crashed;
- the URL left the watched page (a sign-in redirect, a geo-block page, a different path);
- the `settle` element never appeared;
- a `view` element is missing or renamed;
- `pick` found a page that no longer looks like the recipe's (e.g. zero candidate elements where there used to be
  many);
- the gateway refused a step.

The round fixes the recipe, asks the owner (a sign-in, for example), or pauses with a question, as loops do today.

### 5. Where it runs
- **Default:** in FERB's own browser profile, in its minimized window, with no effect on the owner's windows (rule 17).
- **The owner's own tab** when the owner asks ("watch this tab"), held for the whole loop (2026-10-25 loop-speed fix).
- **Sign-in-only pages** use the owner's paired profile, through the browser tools. Sealed logins (L36 login records)
  fill only at their exact place.

### 6. The owner sees and controls it
- The notch's standing-work row: "Watching: Handshake tasks · every 45–75 s · 2 new today" (L31's row).
- `stop` ends it, and a reply steers it, as with loops.
- Each alert names the watcher, and "stop watching" is one reply away.

## Use cases this covers (not per-site code)
- Visa appointment slots: new dates appear.
- Job boards and gig pages: new claimable tasks.
- Restocks: "Add to cart" becomes enabled.
- Prices: a price crosses a number.
- Tickets, flights, course seats, exam slots: a slot opens.
- Application, order or delivery status text changes.
- CI and status dashboards: a build turns red.
- Web apps without notifications: a count goes up.
- Keep-alive: reload every N minutes.

## Not in this design
- Page JavaScript or userscripts. A watcher can't run code inside the owner's pages.
- Automatic actions on a trigger other than through a round with the usual confirmations.
- Native-app watchers. Browser pages first; macOS apps can come later through the same recipe shape with mac tools.

## Testing
- **Recipe validation.** Only the vocabulary is allowed. Patterns are size-limited. A `view` click needs an exact name.
  A floor below 30 s is refused.
- **Cycles.** With a fake bridge: refresh → view (clicks only when unselected) → settle → read → pick → compare. New
  items trigger notify / offer / round; no change records one compact row; there is no AI call (a spy on the runtime).
- **Anomalies.** Each anomaly wakes one round, with back-off.
- **Pacing.** The random interval stays within its range, and the 30 s floor holds.
- **Research.** The first round's plan line carries the rate-limit finding with a source (prompt contract plus a
  scripted-worker test).
- **Lab** (headless lab Chrome, like L33's): a page whose "Claim" buttons appear after N reloads triggers `new_items`
  exactly once per new item. A sign-in redirect is an anomaly.
- **Live, owner-observed:** a real watch on an owner-chosen page (e.g. the visa calendar or a job board), alert on a new
  item, run for an hour with no AI calls between triggers (`ferb stats`).
- L38 in `docs/0.27_DECISIONS.toml`; ARCHITECTURE.md (16.x watchers), README (watchers, polling floor, research),
  CHANGELOG.md.
