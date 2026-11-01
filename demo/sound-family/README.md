# FERB sound family (decided 2026-11-01)

The decided family (owner's picks over nine rounds; decisions in `kb area app`):

- First launch: Google's Material "hero" clip (CC BY 4.0, CREDITS.md), played once; silence until the logo.
- Crystal (synthesised): sound logo, summon, listening start/end, done (bright); needs you (one ringing A);
  paused and error (the damped voice); the orb's drip-off (the old flight sound), hop and melt-back.
- Notification: Material's "notification, high intensity" (CC BY 4.0) for every notification, banners and notch.

`render.py tactile signal bloom` still renders round 3's other directions for reference.

## Render

    uv run demo/sound-family/render.py           # Crystal -> demo/sound-family/out/ (git-ignored)
    uv run demo/sound-family/render.py signal    # one direction
    uv run demo/sound-family/render.py --png     # plus spectrograms

`render.py` is the project file: every sound is synthesised from code (no samples), then leveled and checked against
the brief (loudness, true peak ≤ −1 dBTP, starts on its first sample, ends on zero, energy bands). It prints any file
that fails. `measure.py` holds the meters and also measures any WAV: `uv run demo/sound-family/measure.py FILE.wav`.

## Listen

Open `index.html` (single sounds work from disk; for the orb tour and blind pick run
`python3 -m http.server -d demo/sound-family` and visit http://localhost:8000).

Rendering the first-launch sound needs `ffmpeg` (it reads `source/google-material-hero.mp4`).

## First launch (`out/first-launch/`)

- `ferb-first-launch.wav`: the clip, played once as FERB starts the first time; silence until loading is done; then
  the sound logo (never before the clip has finished)

## Files (per direction, `out/<direction>/`)

- `ref/` stereo reference mix (dry on both channels + room), `dry/` dry mono source, `wet/` room stem
- `ferb-room-ir.wav` the room as a stereo impulse response (the app convolves it after placing the dry sound)
- names per the brief: `ferb-<sound>-<standard|subtle>[-v<n>].wav`, `ferb-logo.wav`, `ferb-orb-<drip|hop|melt>.wav`;
