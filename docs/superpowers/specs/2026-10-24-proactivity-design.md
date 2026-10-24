# Proactivity: FERB offers work — design (2026-10-24)

Status: the owner answered this design's questions on 2026-10-24 (below). It awaits the owner's review before a plan.
Decision id: L37. Built before the public launch, after memory (L36), which it uses.

## Why

The owner's X factor: "ferb is approaching me by itself… should keep working and let me feel and know that ferb is here.
should take up work, offer me etc". A read of the code and the live database on 2026-10-24 found the following:
- **Notices only tell.** `attention.md` and `trail.attention` (`ferb/decide/questions.py:70`) forbid tasks, and a
  notice has no action. 109 notices in 14 days were all warnings or status, and none led to a task.
- **Nothing records the owner's reaction.**
- **Notices exist only with a hosted Jev key.** Without one the batcher returns early (`ferb/sense/batches.py`), so a
  new user gets none.
- **Banners are thin.** They go through osascript, only when no client is connected, with no buttons.
- **No mail or calendar input,** and no morning presence.

## Owner decisions (2026-10-24)
- **Offer only.** FERB does nothing for a noticed thing until the owner taps **Do it**.
- **Jev is a switch.** A user with a Jev key may give it, and Jev does the first yes/no filter as today. Without a key,
  the Jev step is left out, and FERB's own AI service looks at activity directly.
- **No limit** on how often FERB approaches the owner.
- **Sources:** a **daily brief**, **Mail** and **Calendar**, on top of today's screen, browser and notification
  activity.

## Rulings in this design (the owner can correct them)
- **Without Jev, batches are considered every 10 minutes per app.** Only apps with new activity count, and an urgent
  pattern (a sign-in code, a security warning) is not held back. Why: keeping the minute cadence on the user's own AI
  plan multiplies its use about tenfold. *Open for the owner: the cadence.*
- **The daily brief arrives at the owner's first activity after 05:00 local,** not at a fixed clock time. A setting
  turns it off.
- **Notices and offers go to the separate Noticed list,** with an eye count in the notch (owner, 2026-10-24). Loop
  pause notices stay in chat (earlier ruling).

## Design

### 1. Offers
- **What the attention step returns.** It may now return `{tell, text, quotes, source_refs, offer}`.
  - `offer` is `{label, goal}`: a button label of up to about 6 words ("Reply to Rahul") and the task goal FERB would
    run, in FERB's words.
  - The prompt (`attention.md`) gains: offer when there is a concrete task the owner would plausibly want done; tell
    when it is information only.
  - `questions.py:70`'s effect line changes to "a notice or an offer; never an action".
- **The notch.** It shows an offer in the Noticed list with **Do it** and **Not now**.
- **Do it** calls `notice.accept {id}`. The daemon starts a chat turn for that session whose owner part is fixed text
  written by code: "Do this offer from FERB: “<label>” — <goal>".
  - The planner plans it as an ordinary request. The offer's text carries FERB's INFERRED origin plus its sources'
    origins (external content stays data, MP2).
  - The tap authorizes only starting the task. Every code confirmation still applies (L3: money, account security,
    permanent deletion, a new recipient, the sink rule S4).
  - A destination that came from outside content is never "already authorized" by the tap (rule 8 provenance:
    the owner's typed words are what count).
- **Not now** dismisses the offer.
- **Expiry.** An offer whose sources are older than 24 hours shows "may be out of date".

### 2. The Jev switch
- A setting `[attention] jev = "auto" | "off"`, default auto: use Jev when a key exists.
- `ferb setup --jev` (and later Settings → AI) adds or removes the key.
- **Without Jev,** `batches.py` stops returning early. Batches go straight to the orchestrator's attention
  consideration (`Supervisor.attention`), merged per app over the 10-minute window. The existing "one consideration
  per app at a time" stays.
- `ferb doctor` and the notch status say which path is active.

### 3. No cap, but no repeats
- No rate cap (owner).
- The attention step already sees what it told for that app. It also gets the owner's recent reactions (section 4),
  and offers the same thing again only when the owner hasn't dismissed it.

### 4. Reactions are recorded and fed back
- **What gets recorded.** `attention_notices` gains `offer_json`, `outcome` (accepted | dismissed | ignored), and
  `outcome_at` plus `task_id` when one was accepted.
  - "Ignored" means not acted on within 24 hours.
  - Recording is code-only and keeps no extra text.
- **Feedback.** The attention prompt sees the last 20 reactions for that app (label and outcome), so it learns what's
  worth offering.
- **Stats.** `ferb stats` shows offers made, accepted, dismissed and ignored.

### 5. New sources: Mail and Calendar
- **Mail.**
  - A watcher reads new messages' sender, subject and date (no bodies) from Mail's own index database. It is
    read-only, through Full Disk Access, which FERB already has, and needs no app automation, so Mail is never driven.
  - It runs every 2 minutes.
  - Rows enter the trail as `mail.new`, after the existing exclusion and redaction. A body is read only when a task
    asks for it.
  - Everything from mail is external content: data, never instructions (MP2).
- **Calendar.**
  - FERB.app reads upcoming events (next 36 hours: title, time, location, attendees count) through EventKit, after the
    macOS Calendar permission prompt (the setup window's Permissions step, L34).
  - It writes them to the daemon over the app's RPC every 15 minutes and on change.
  - Rows enter the trail as `calendar.upcoming`.
- **Both feed attention like other trail sources,** so meeting prep, a reply waiting or a delivery can become an offer.

### 6. The daily brief
- At the owner's first activity after 05:00, code assembles the day's facts:
  - today's reminders and schedule runs;
  - open and paused tasks and running loops;
  - today's calendar events;
  - new mail since yesterday evening (count and senders);
  - yesterday's notices and unanswered offers.
- The orchestrator writes it in 3–6 short lines, with at most 3 offers attached (same offer mechanism).
- It goes to the chat and as a notification. It is skipped when there is nothing to say.

### 7. Presence in the notch and notifications
- **The notch's idle line** shows what FERB is keeping or watching ("Visa check 10:00 · 1 loop · 2 noticed"). It reuses
  the standing-work row (L31) and the Noticed count.
- **Notifications.** FERB.app posts notices, offers and the brief as real notifications with **Do it** / **Not now**
  actions (L34 Phase D). The daemon falls back to osascript only when the app isn't running.

## Not in this design
- Doing anything before the tap: no preparation, drafts or reversible actions (owner: offer only).
- Rate caps or quiet hours (owner: no limit). macOS Focus still governs notification banners, as macOS does for every
  app.
- Reading mail bodies or sending mail from an offer without the task's normal confirmations.

## Testing
- **Offers.** The attention step's offer is stored and shown. `notice.accept` starts exactly one task with the fixed
  owner part. A tap on an offer from external content still triggers the sink rule for an outside destination, and a
  payment still asks. Not now records dismissed. Ignored is set after 24 hours.
- **Jev switch.** With no key, batches reach the orchestrator after 10 minutes per app (fake clock), and a sign-in-code
  row isn't held back. With a key, Jev gates as today. Doctor shows the path.
- **Mail.** A fixture copy of Mail's index yields `mail.new` rows (sender, subject, date only) through redaction and
  exclusion. A missing or unreadable index gives a clear doctor line.
- **Calendar.** The RPC accepts events and writes `calendar.upcoming`. Swift: the EventKit reader maps events, and
  permission denied shows the reason.
- **Brief.** It fires once at the first activity after 05:00, is skipped when empty, and carries at most 3 offers.
- **Live, owner-observed.** An offer from a real email or a calendar event, tapped Do it, runs. A brief arrives the next
  morning.
- L37 in `docs/0.27_DECISIONS.toml`; ARCHITECTURE.md (attention, offers, sources), README (proactivity, Jev optional,
  privacy of mail and calendar), CHANGELOG.md.
