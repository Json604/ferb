# FERB's sound (research 2026-11-01, for notes/setup-intro.md "Sound")

Status: family decided 2026-11-01 (see "Rounds so far"); rendered by `demo/sound-family/render.py` and heard on
`demo/sound-family/index.html`. In the app (setup-intro plan Task 10, 2026-11-01): the tour and every everyday event
play through one FerbSoundEngine, which applies each sound's switch (`feedback.sound.<name>`), Standard/Subtle and the
call rule; banners carry `ferb-notification-banner.wav` (render.py's stereo reference mix, copied into
FERB.app/Contents/Resources by build-app.sh); the magnetic docking sound is gone; the CC BY credit is in Settings →
About. Next: the open decisions at the end. Sources are cited in the research report (session of 2026-11-01); the key ones are kept inline
here.

## Rounds so far
- Round 1 (placeholder sines, E5–B5–G#5 arpeggio, noise whoosh, pitch-drop settle): owner, "not good".
- Round 2 (`demo/sound-audition/`, modal synthesis in five directions): none liked; motif I "Open fifth" liked a bit.
  Owner's words on why: "the swoosh sounds. sounding like cheap sounds, not premium and something with a feel to it".
- Round 3 (`demo/sound-family/`): the owner asked to hear four product sound worlds (Apple Pay/AirPods, Google Pixel,
  Nothing/Teenage Engineering, warm soft chords). Built from measurements of the Mac's premium cues
  (payment_success, jbl_confirm, begin_record, Glass: one to three clean tuned partials, a strike gone within ~50 ms,
  clean decays, at most a rhythmic echo; no noise, no wash). Directions: Crystal, Tactile, Signal, Bloom; one shared
  score on the Open-fifth seed; error became a falling minor third ("uh-oh"), paused a single thinning note; orb flight
  is a tonal trail, never air. Every file is rendered offline at 48 kHz/24-bit as dry mono, room stem, stereo reference
  and room IR, Standard and Subtle, 3 variants where the brief asks, and checked against the brief's spec (loudness,
  true peak, clean start/end, band limits). Synthesised by Claude from code: see the ownership question below.
- Round 3 verdict (owner, 2026-11-01): **Crystal** for the logo, summon, listening start/end, done, orb flight, hop and
  melt-back. Rejected: needs you ("needs to [tone] it down a bit to show that it needs you"); paused, error and needs you
  "all sound like a family of completion. they shouldn't, they should sound dull"; drip-off ("nothing gave the leaving
  feel"); landing. Also: "i dont want too much noise everywhere too. lets have it for big events and only where its
  necessary."
- Round 4 (`demo/sound-family/`, Crystal only): needs you, paused and error use a damped Crystal voice (same glass,
  finger-damped: body and note only, short, an octave below the bright cues, no strike); needs you 1 dB quieter
  (−23). Three options each for needs you, paused, error, drip-off and landing, picked on the page. Proposed default
  sound map (owner to confirm): sound for needs you, done, error, listening start/end and the tour; summon and paused
  silent by default; everything switchable.
- Owner, 2026-11-01 (after round 4): no orb flight sound; the drip-off sound is what plays when the orb sets off and
  the melt-back what plays when it lands (landing and flight cut as separate sounds). FERB's first sound is a Google
  Material Design "hero" clip the owner chose (`demo/sound-family/source/google-material-hero.mp4`, CC BY 4.0 per a
  redistributing project; licence confirmed by the owner, 2026-11-01): it plays before the sound logo while FERB loads
  the first time (first extended to last as long as loading; replaced in round 8, below). This supersedes the
  brief's "no third-party material / full ownership" for this one sound; CC BY needs a credit shipped with FERB
  (`demo/sound-family/CREDITS.md`). The clip is G major; into the logo's D it is a plagal resolution, no retuning.
- Owner, 2026-11-01 (round 6/7): drip-off *is* the old flight sound (singing glass, one 1.2 s swell), not a new
  sound. The first-launch loop must not sound the same on long loads: it is now ~36 s of four waves of different
  heights (level and brightness rise and fall with the clip's own build), with an optional faint-glint version.
  Paused = round 4 option 1 (one damped tap). Error = option 1 ("uh-oh"), made duller and sadder (slower, sinking onto
  B3 + F#4, a hollow B minor). Needs you: the installed build's sound (macOS "Basso") is "not good"; the owner asked to
  find a better one online: three candidates from Google's Material sounds (CC BY 4.0, in D major like the family).
- Owner, 2026-11-01 (round 8): the extended first-launch loop (waves, with or without glints) rejected as "too much
  sound". The clip plays once at the start, then no background sound until the notch's sound logo; the logo waits for
  the clip to finish if loading is quicker.
- Owner, 2026-11-01 (round 9): FERB's notification sound (its macOS banners; `NotificationPoster.swift` posts them with
  `content.sound = .default` today) is Material's "notification, high intensity" (CC BY 4.0). Needs you: the Material
  candidates were not it either; wanted "one tone, conveying that need": three one-note options on A (the dominant,
  unresolved): one ping, tap-tap, calling (with two echoes).
- Owner, 2026-11-01: every notification FERB gives uses that notification sound: its macOS banners and the
  reminders, schedules and notices that arrive through the notch chat. Today those notch notices are silent
  (`DaemonConnection.receiveNotices` adds them without a cue) and banners use `.default` (`NotificationPoster.post`).
  For the integration step: one notification cue for both paths, played once when a notice is shown both as a
  banner and in the notch (the banner carries it as `UNNotificationSound(named:)` with the file in the app bundle).
- Owner, 2026-11-01: needs you = round 9 option 1, "One ping" (one Crystal note on A, left to ring). The family is
  decided; `demo/sound-family/render.py` renders it (`out/`, git-ignored). Integration into the app (replacing
  SoundFeedback.swift's system sounds, the notification cue, per-sound controls) is the next step.

## Why the first placeholder sounded cheap
- Pure sine plus harmonic partials sounds like no material. Real glass, metal and wood have inharmonic partials, a
  sharp transient, and highs that decay faster than lows.
- A major-chord arpeggio (E5–B5–G#5) is a notification jingle; simple harmony tolerates repetition better (Will
  Littlejohn, 20K "Facebook & Google Pixel").
- A filtered-noise whoosh and a pitch-drop "settle" are stock effects; the pitch drop reads as a cartoon power-down.

## What premium sounds are made of (measured on this Mac)
- `/System/Library/Sounds/Glass.aiff`: fundamental ~390 Hz; upper partials in tight clusters a few Hz apart (the
  shimmer is their beating); brightness falls from ~7 kHz to ~1.5 kHz in 0.5 s (a bright strike that mellows).
- Siri (`/System/Library/Components/CoreAudio.component/Contents/SharedSupport/SystemSounds/siri/jbl_*.caf`): one warm,
  low timbre with near-identical pitch pairs (~242/261 Hz, ~331/350 Hz); begin = swell (1.22 s, ~−22 LUFS), confirm =
  short note (0.58 s, ~−27), cancel = falling fifth (1.96 s, ~−19). The contour carries the meaning.
- Small system cues (`.../SystemSounds/system/`): `begin_record`/`end_record` ~0.45 s at −33 to −36 LUFS (a matched
  pair, very quiet); `payment_success` 1.41 s, ~−28 LUFS, bright octave with air.
- Recipe: a material resonance with inharmonic partials, a 2–5 ms transient, micro-detuned pairs, a small room, a
  high-pass around 120–200 Hz, harshness tamed at 2–5 kHz, files trimmed at zero crossings (HeyClicky shipped clicks
  and a "crack" in its chimes, later fixed).

## The family from one seed
- One material, one pitch set (a 5-note scale, so any subset is consonant), one room, one mastering chain; children
  change only contour, rhythm and length (Brewster's earcon guidelines). Cut the everyday sounds down from the sound
  logo rather than composing new ones (Antfood: Instacart, YouTube).
- Roles, lengths and levels (starting points, by ear on MacBook speakers at a fixed volume):

| Sound | Feels like | Length | Level | Visual twin |
|---|---|---|---|---|
| Intro / sound logo (tour only) | "Hello, this is FERB": the whole motif | 1.5–3 s | ~−20 LUFS | notch swell, orb birth |
| Summon | "I'm here": light, quick, upward | 150–400 ms, within 100 ms of the key | ~−28 | notch opens |
| Listening start | "Go ahead": open, soft, no hard attack | 200–500 ms, done before the mic opens | ~−30 | breathing pulse |
| Listening end | "Got it": the mirror of start | 200–400 ms | ~−30 | pulse stops |
| Working | silence (FERB works in the background) | none | — | subtle notch shimmer |
| Done | "Done, all good": falls to the home note | 300–700 ms, on real completion | ~−26 | check + summary |
| Needs you | "Over here": unresolved, a little brighter; loudest everyday sound | 600–1200 ms, at most one repeat | ~−22 | notch tints, stays open |
| Paused | "Holding": a downward, thinning swell | 300–500 ms | ~−30 | dimmed notch |
| Error | "That didn't work": dry, gently falling, never harsh | 400–800 ms | ~−26 | error state |
| Orb motion (tour) | drip-off pluck, quiet air in flight, landing tick, melt-back swell to the home note | keyframe-synced | quiet | the orb |

- Hierarchy (loudest, rarest first): intro; needs you; done, error; summon; listening, pause; motion ticks.
- Frequent sounds vary a little per play (±10–20 cents, ±1–2 dB, 2–4 variants; Apple, WWDC23 "Explore immersive sound
  design"); the signature never varies.
- No loop while listening or working: a loop leaks into the microphone and breaks "calm once settled".

## Motion and space
- Discrete sounds on the orb's motion keyframes plus one very quiet air layer whose level and brightness follow the
  orb's speed (the Mini Metro principle: events pitched to the motif's scale, throttled). No Doppler (on-screen travel
  is ~30 cm; pitch bends read as cartoon).
- MacBook speakers convey left/right, not up/down (generic HRTFs localise elevation poorly even on headphones). Map
  screen x to pan (full width ≈ ±0.6, the notch at 0); show distance from the notch with a small level drop, low-pass
  and a little more room, not elevation. HRTF only when headphones are the output (AVAudioEngine +
  AVAudioEnvironmentNode; PHASE is more than FERB needs). Head tracking (macOS 15+, entitlement) deferred.
- Bluetooth adds ~125–170 ms: keep the engine warm, pre-roll, and on Bluetooth start the sound early or delay the
  visual by the measured device latency (`presentationLatency` often reads 0; sum Core Audio's latencies).
- System settings: AVAudioEngine doesn't honour "Play user interface sound effects" or the alert volume by itself; read
  them (`com.apple.sound.uiaudio.enabled`, alert volume) and honour "Play stereo audio as mono" and "Flash the screen";
  meaning never depends on position.

## Context facts the code can read (the rules are the owner's)
- Another app using the microphone (calls, dictation): Core Audio process objects, `kAudioProcessPropertyIsRunningInput`
  (macOS 14+, PID available; apps set it during pre-join previews too).
- Focus: `INFocusStatusCenter` (on/off only, with authorisation).
- Screen sharing: no public API (heuristics only).
- Media playing: per-process output state (unverified); MediaRemote is entitlement-locked since macOS 15.4.
- Late night: the clock.

## Four audition directions
- **A. Glass and light** (matches the orb): struck and rubbed glass, micro-detuned shimmer, bright strike that mellows.
  Risk: shrill or too close to Apple's Glass.
- **B. Liquid**: droplet resonances over a soft body (drip-off, melt). Risk: cartoon pops, messaging-app feel.
- **C. Felt and wood**: felt piano, marimba, kalimba; the warmest and most repetition-tolerant. Risk: homely next to a
  glass visual.
- **D. Breath and air**: breathy noise and a soft hummed formant. Risk: weak on laptop speakers, too close to Siri.
- Research's expectation: A with D's air layer underneath.
- Judging: blind A/B on MacBook speakers at 50% and on AirPods, in context (orb animation, quiet music underneath), a
  50-repeat loop of done and needs-you, living with the top two for 2–3 days each, re-listening on another day.
  Then the recognition test with 2–3 beta friends (each sound matched to its meaning, ≥ 80%), a week of daily
  annoyance ratings, context and accessibility checks; lock the family before commissioning.

## Production route (research recommendation: hybrid)
- Prototype in the browser with real synthesis to pick the direction and the motif; then commission a UI-sound
  specialist (individual or small studio, ~€2k–15k tier; studios €15k–60k) for the final family, with the prototype as
  the reference.
- Prototype technique: modal synthesis (6–10 resonators, inharmonic ratios, micro-detuned pairs, faster-decaying highs)
  excited by a 2–5 ms filtered-noise mallet or slow-attack noise for rubs; an air layer; a small-room convolution
  reverb; gentle saturation; high-pass ~150–200 Hz; nothing essential below ~250 Hz; WAV export of candidates.
  Libraries: `modal-synthesis` (MIT), Faust `physmodels` via faustwasm, webmi (Rings/Elements, MIT).
- Licences: synthesise from own patches (Logic's Sculpture output is fine; avoid bundled loops and Kontakt/NI library
  content, which forbid redistribution as sound effects); OpenAIR impulse responses vary per file.
- Commission contract: work-for-hire plus a fallback copyright assignment of composition and masters; stems and project
  files; 48 kHz/24-bit WAV; no third-party samples; explicit permission to publish the files in the open-source repo;
  revision rounds stated.

## Owner decisions (2026-11-01, after the rounds)
- The family ships as FERB's own sounds: the Crystal set is synthesised from code under the owner's direction, with no
  samples; hiring a designer stays optional for later (supersedes "no purely AI-generated signature").
- Default on: needs you, error, done, notification, listening start/end, the tour. Default off: summon, paused. Every
  sound has its own switch.
- The magnetic docking sound (MagneticDockingAudio.swift) is removed.
- Integration is in notes/setup-intro-plan.md (Task 7 for the tour, Task 10 for everyday sounds).
- The Material clips' licence (CC BY 4.0, credit shipped with FERB) is fine: owner, "the material license is green".
- Standard is the default intensity; Subtle is the choice in Settings. The master volume stays at today's 0.18.
- Quiet during calls only: while another app is using the microphone, FERB plays no sound except needs you, at Subtle;
  the notch still shows everything. Focus, quiet hours and screen sharing get no rule (macOS already silences FERB's
  banners in Focus; reading Focus would cost a permission). Owner chose this after the reasons were explained.

## Owner decisions still open
