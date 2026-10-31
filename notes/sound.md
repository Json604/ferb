# FERB's sound (research 2026-11-01, for notes/setup-intro.md "Sound")

Status: research done; next is a browser audition of four directions with the owner, then production (hybrid route,
below). Owner decisions still open are listed at the end. Sources are cited in the research report (session of
2026-11-01); the key ones are kept inline here.

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

## Owner decisions still open
- Which direction (after the audition).
- Commission or in-house for the final family, and the budget.
- The licence of the sound files in the public repo.
- Context rules: quiet hours, whether calls/meetings mean silent or quieter, Focus behaviour.
- Customisation defaults (master FERB volume, per-sound toggles, a Subtle/Standard intensity).
