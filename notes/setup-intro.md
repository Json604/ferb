# First-run intro and setup (design, 2026-11-01)

Status: design agreed in conversation with the owner on 2026-11-01; the sound family was decided the same day over
nine listening rounds (notes/sound.md). Tasks 1–11 built 2026-11-01 (notes/setup-intro-plan.md); live checks pending
(step 13). Replaces today's setup window flow (fix-order step 15; it comes before step
13's fresh install). Visual polish happens in the Swift app (owner: "needs polishing and that we'll do in the swift app").

## Intent (owner's words, condensed)
First install should feel like *meeting FERB*: not a prettier validation screen or a checklist of permissions, but a
short, memorable, interactive introduction that makes FERB feel like it belongs on the Mac. Teach through small
interactions, not slides. Keep the real FERB interface (the notch) central throughout, so setup feels like using the
product. Capable and warm, not cartoonish or theatrical. Permissions are guided with clear visual demonstrations,
including drag-to-add, so nobody guesses their way through System Settings. Sound is part of FERB's identity (below).
Inspiration: HeyClicky's engaging intro (principles only, never its assets or flow verbatim); the motion language of
Apple's Dynamic Island; the macOS 27 Siri orb's liveliness (one object that morphs; content grows out of it; a light
sweep as it arrives; calm once settled).

## Goals
- A new user meets FERB, connects it, sees what it can do on *their* Mac, learns to control it and to bring it back,
  in one continuous tour that ends in the notch.
- Every required grant is done by the end, guided, with each grant confirmed from the system (never assumed).
- The user trusts FERB before it ever acts: the demo looks and tells, it never acts.

## Non-goals
- No new FERB capabilities; the intro shows existing ones.
- No account system, no marketing screens, no feature slides.
- Final sound assets (they come from the audition and production route the sound research recommends).

## Anti-goals (a failure even if it works)
- A wizard of screens with Next buttons; a permission checklist dressed up.
- A separate "onboarding app" look that isn't the notch.
- Anything that acts on the user's Mac during the demo.
- Music under the tour; sounds that annoy by the tenth hearing.
- Leaving the user stuck in System Settings, or claiming a grant that the system doesn't confirm.

## The direction: "C", the notch speaks, an orb points (owner, 2026-11-01)
Chosen after trying throwaway prototypes A (the notch grows and reaches with an attached arm), B (a free companion
orb) and C (the mix). Owner: "i like the mixture of a and b. the notch it is. BUT i dont want the black line, rather the
orb flying when to show the permission thingy."
- **The notch is FERB's home and voice.** It swells down out of the notch to speak and shrinks back when done. All of
  FERB's words in the tour come from the notch, except a short instruction beside the orb (below).
  FERB's lines are text in the notch, never read aloud; sound in the tour comes only from the sound family
  (owner, 2026-11-01: text + sound cues).
- **The orb points.** When FERB needs to show a place on screen (a System Settings list, a browser prompt, an app it
  would use), a small glass orb drips off the notch (a liquid split), flies there, helps with the step, does a small
  happy hop when the step is done, flies home and melts back into the notch; the notch swells once as it takes it back.
  The orb is glass with an iridescent band of light inside that ripples while FERB listens or waits.
- **Instruction beside the orb** (ruling: the user's eyes are on the orb at that moment): a small frosted bubble next to
  the orb with one line ("Drag me into this list") and a reason line.
- **Motion**: springs with perceptual duration ~0.35–0.5 s; opening with a small bounce (0.15–0.3), closing and
  dismissal critically damped, morphs between states snappy; every animation interruptible and retargetable mid-flight
  (springs keep velocity). Decorative animations ≤ 2 s; the user never waits on an animation. Content stays concentric
  with the notch (no "forehead"). Liquid splits and merges (metaball style) between the notch and the orb.
- **Sound follows FERB** (spatial, below): at the notch while FERB speaks; with the orb while it's out; home with it.
- Prototype for reference (throwaway, not to ship): the brainstorm companion page "intro-c.html" (2026-11-01).

## The journey (owner, 2026-11-01: "yes yes, this is what i meant"; order agreed)
1. **Meet.** The notch stirs, swells down, and FERB says hello in one or two lines. The full sound signature plays once.
   Before sign-in, FERB installs its background service silently during Meet; if macOS asks for the Login Items
   approval (`serviceNeedsApproval`), the orb guides that one System Settings visit first (ruling: the daemon must run
   before sign-in).
2. **Sign in.** Connect a model: Codex or Claude first, with "Other ways to connect" for an Anthropic, OpenAI or Google
   API key (ruling: CLAUDE.md rule 12 keeps the API runtimes first-class). As soon as sign-in succeeds, FERB starts the
   Mac scan in the background (memory decision 16: installed apps, their use, browsers and profiles; code gathers, a
   model interprets), so it is ready by step 6.
3. **Allow all, guided** (keeps the owner's decision that all seven permissions are required with one Allow all;
   Automation stays per app at first use). Pressing Allow all starts the sequence:
   - drag-list panes (Accessibility, shown on macOS 27 as "Device Control and Data Access"; Screen & System Audio
     Recording; Input Monitoring; Full Disk Access; whichever of today's seven are list panes): FERB opens the exact pane
     (`x-apple.systempreferences:` deep links), the orb flies out and docks beside System Settings holding a real,
     draggable FERB icon, follows the Settings window if it moves, hides when Settings isn't in front, and confirms only
     when the system reports the grant;
   - request-style permissions (microphone, notifications, and any other of the seven that macOS asks with its own
     prompt): one line from the notch, then the system prompt where the user is;
   - a grant that needs FERB to restart (Screen Recording) restarts the part that needs it automatically when the user
     comes back, without losing the tour's place.
4. **Pair all browsers** (owner, 2026-10-26: the extension is added to every installed browser; a Pair all button pairs
   everything at once, "so there's no manual step later"; a per-profile Pair stays for those who want it). The orb flies
   to each browser's own "turn on" prompt if one appears.
5. **FERB's own email, optional** (owner: optional, preferred). One beat: "Want me to have my own email for sign-ups and
   searches? Set it up now, or later." Skipping costs nothing; it stays in Settings and FERB asks again the first time a
   task needs it.
6. **Try: look, tell, show the journey** (owner, 2026-11-01: "look and tell it can do it. NOT ACTUALLY DO IN THE DEMO.
   BUT SHOW the journey that it can do and the visuals in the chat too").
   - FERB says what it saw from the scan in one or two specific lines ("here's what I saw").
   - It suggests two first tasks that fit this user, ideally of different kinds (e.g. one on the Mac or in files, one
     in the browser); if the scan found nothing clear or isn't ready, two solid general tasks of different kinds
     instead (owner, 2026-11-01: more than one; two suggested, one previewed). The user picks one; a small "Show the
     other one" plays the second preview for anyone curious.
   - It shows the journey of the picked task without doing it: the plan and the answer-in-steps rows and cards in the real
     chat UI, exactly as a real task looks, each step clearly marked as a preview; the orb flies to the apps or windows
     it would use. Nothing is clicked, typed, sent, opened or changed.
   - It ends with a "Do it for real" choice for the previewed task, which the user may take or skip. Ruling: a task taken here starts as a normal
     real task (its usual questions) right after the tour ends in step 8, so the tour itself never runs real work.
7. **Control.** Taught by doing, with today's controls: double-tap right ⌥ opens or closes FERB; hold to talk
   (push-to-talk, sends when the keys are released if that setting is on); Control-Option-Space also opens it; how to
   stop a task and pause FERB. The user does each once; stopping is practised on the step-6 preview, replayed as a
   running preview (ruling: no real task runs during the tour).
8. **Back to the notch.** The tour ends with FERB small in the notch and one line on how to bring it back. Replay the
   intro from Settings.

## Sound (owner, 2026-11-01; family decided, notes/sound.md)
Owner's words, condensed: an original, recognisable family of sounds, not unrelated effects. Intro, summoning,
listening, completion, needing attention and pausing share one sonic character while saying different things.
Recognisable without looking at the screen. Setup plays the fuller signature; everyday sounds are shorter, quieter and
pleasant after repeated use. Spatial audio brings the tour to life: sound is attached to FERB's position and movement,
not a soundtrack. Visuals, motion and sound are designed together; the experience is fully understandable with sound off.
The owner judged the prototype's synthesized placeholders "not good" and asked for dedicated research; they shared
Fares Farhan, "Sounds in User Experience" (Medium, 2023) to explain the intent: sound as a designed UX system.
- **A system, not effects**: each sound means one thing (affordance); immediate feedback; consistency across the family;
  a hierarchy (needs-attention loudest and rarest; most sounds subtle); context sensitivity (quieter or silent during
  calls, meetings, screen sharing, Focus, media playing, late at night); accessibility (every sound has a visual twin);
  a sound logo that seeds the family; user control (volume, per-sound on/off); tested by living with it (the owner and
  2–3 beta friends).
- **One family from one seed**: the sound logo (full, intro only) and short derived cues for summon, listening,
  done, needs attention, pause (and error), quieter and shorter the more often they play.
- **Spatial in the tour**: mono sources positioned from the notch's and the orb's on-screen frames (an
  AVAudioEnvironmentNode or the simplest technique the research supports), with a stereo-pan fallback on built-in
  speakers; compensate output latency; no tight sync promised on Bluetooth.
- **Honour the system**: the "Play user interface sound effects" setting and the alert volume; never music under the tour.
- **Ownership** (owner, 2026-11-01): the Crystal sounds, synthesised from code under the owner's direction with no
  samples, ship as FERB's own; a hired designer stays optional for later. The two Material clips are CC BY 4.0 and
  ship with their credit. No library loops, no game or film music.
- **Scope** (ruling from the owner's plan, which covers everyday sounds): the whole family is in this step, in two
  parts in notes/setup-intro-plan.md: the tour plays the decided family (Task 7), and the family replaces today's system
  sounds (SoundFeedback.swift: Tink, Pop, Bottle, Glass, Basso) with per-sound switches (Task 10). Defaults (owner,
  2026-11-01): on for needs you, error, done, notification, listening start/end and the tour; off for summon and
  paused; Standard intensity. The magnetic docking sound is removed. Context: during a call (another app using the
  microphone) only needs you plays, at Subtle; no Focus, quiet-hours or screen-sharing rule (all owner, 2026-11-01).
- **Process** (done 2026-11-01): four rounds of directions, then the Crystal family refined over five more rounds;
  `demo/sound-family/render.py` renders it, `demo/sound-family/index.html` plays it.
- **The decided family** (owner, 2026-11-01; details and levels in notes/sound.md):
  - First launch: Google Material's "hero" clip (CC BY 4.0, credit ships with FERB), played once as FERB first
    starts; then silence until the notch plays the sound logo, which never starts before the clip ends.
  - Crystal (bright glass): the sound logo (Open fifth, tour only), summon, listening start and end, done.
  - Crystal, finger-damped (dull, an octave lower, no strike): paused (one damped tap) and error (a sinking "uh-oh").
    These must not sound like completion.
  - Needs you: "One ping", a single Crystal note on A (the unresolved dominant) left to ring; the loudest everyday cue.
  - Notification: Material's "notification, high intensity" (CC BY 4.0) for every notification FERB gives, on macOS
    banners and on notices in the notch chat alike, played once when a notice shows in both places.
  - The orb: drip-off when it sets off (one singing-glass swell), hop when a step succeeds, melt-back when it
    arrives at its target and when it comes home. There is no flight sound and no separate landing sound.
  - Sound only for big events and where it's needed (owner: "i dont want too much noise everywhere").

## Constraints
- A resumed tour whose background service is down waits on "Getting my background part ready…" until the daemon
  answers or the owner skips (ruling, 2026-11-01: intended; the tour never claims a grant it couldn't read).
- CLAUDE.md rules: the app is the product (rule 13); background first after setup (rule 10: the tour is the user's own
  explicit flow, but nothing in it moves the user's cursor or types for them); trust follows origin (rule 9); code
  enforces boundaries, never a model (rule 3): the demo in step 6 runs without any acting tool available, so "it never
  acts" holds by construction, not by instruction.
- Permissions: all required (owner decision); each grant confirmed from the system with a real check where the plain
  API can be stale (Accessibility: a real AX call; Screen Recording: a real capture; Input Monitoring: IOHIDCheckAccess);
  polled live (~1–1.5 s); never inferred from a drop. Required grants missing at the end bring the setup back at next
  launch, as today.
- Reduce Motion: springs become cross-fades; the orb fades in place beside its target instead of flying. Sound off:
  everything readable. "Play user interface sound effects" off: no UI sounds.
- Notchless Macs: the virtual notch pill (owner decision 2026-10-27) plays the notch's role.
- Skippable at any point, resumable where it was (quitting mid-tour resumes at the same beat), replayable from
  Settings. Skipping the tour leaves the required steps (sign-in, permissions) in a compact form; a skip tears down
  everything the tour drew or said. The app stays alive during the tour (opt out of automatic termination).
- Upgrades: an existing install that has finished setup doesn't see the tour (it is offered in Settings as "Replay
  intro"); one that finished setup and is missing grants gets only those beats plus home. An install that never
  finished setup and predates the intro is treated as a first run (the full tour; ruling, 2026-11-01).
- Launch (built, Task 9; AppSetupPlan.atLaunch/afterUpdate): a new install or another installation → every beat; a
  quit tour → its own plan at its beat (IntroProgress keeps the plan; it wins over the compact window's Grant resume);
  setup done → nothing, except that a quiet update finding a required grant off runs Allow all and home only; setup
  not done and the tour never seen → every beat (a first run); setup not done after the tour → the missing beats (a fact the daemon didn't answer asks for nothing; browsers count as unpaired only
  when profiles exist and none is paired), and with none missing after the tour, launch records setup quietly
  (`setup.finish`; the daemon checks every row). Set Up FERB… and Settings → General → Replay intro run every beat;
  the menu's Restart finding no service repairs it in the compact window, never the tour. A skip, or a grant still
  off at Home, opens the compact window on the first required step left (service, sign-in, permissions).
- A required grant counts as missing after an update for FERB.app's own rows too (Notifications, Microphone): every
  permission is required (owner 2026-10-26) and the daemon's `setup.finish` checks them all (ruling, 2026-11-01).
- The repo stays general (no personal data in examples, copy or tests).
- The owner polishes visuals in Swift; this design fixes behaviour, flow, motion rules and the sound system.

## Left to the builder
- Exact copy for each beat (warm, plain, short; FERB's voice), within the beats above.
- The orb's rendering technique, the metaball split/merge, and the drag card's implementation (open-source references:
  AskForPermission (MIT) and Scrawl (MIT) for the docked draggable card and live grant detection; study, don't copy
  assets).
- How the preview task in step 6 is produced (a plan-only run with no acting tools), and how its rows are marked as a
  preview. Built (Task 4, daemon): `intro.suggest` (one tool-less call on the redacted scan summary for two
  suggestions; the two general tasks when the scan isn't "seen" or the call fails or stalls past `SUGGEST_LIMIT_S`,
  just under the app's wait) and the stream `intro.preview` (`Supervisor.preview`: the real planner turn alone, no run
  token, worker, task row or chat entry; its context leaves out the owner's live state: running tasks, open questions,
  earlier chat, notices, schedules and reminders, keeping the capability facts (paired browsers, worker types), recalled memory and today's activity, as a real plan would use;
  its session prefix `preview:` is one `chat.send` refuses; events `plan`, `part` per step, `done`, all
  `preview: true`), both FERB.app only.
- Which of today's seven permissions are list panes vs prompts, and their deep links on macOS 14–27.
- The internal structure of the setup code it replaces, and tests (focused: grant confirmation logic, resume/skip state,
  the preview run having no acting tools, upgrade behaviour).
