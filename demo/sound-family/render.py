"""FERB sound family: renders every sound of the brief in four directions, then levels and checks them.

    uv run demo/sound-family/render.py            # all four directions -> demo/sound-family/out/
    uv run demo/sound-family/render.py bloom      # one direction
    uv run demo/sound-family/render.py --png      # also write spectrograms (out/<dir>/png/)

Everything is synthesised from code (no samples), so this file plus measure.py are the project files: change a number,
re-render, and every variant, intensity and stem follows.

Design, from measuring the Mac's own premium cues (payment_success, Siri's jbl_confirm, begin_record, Glass): they are
built from one to three clean, precisely tuned partials, a bright strike that dies within ~50 ms, clean exponential
decays and, at most, a rhythmic echo. No noise layers, no whooshes, no reverb wash. All four directions follow that;
they differ only in material:

  crystal  bright and glassy (the Apple Pay / AirPods world): a ringing note over a warm body an octave down.
  tactile  real small objects (the Pixel world): a soft knock of wood or ceramic with a short pitched ring.
  signal   designed electronics (the Nothing / Teenage Engineering world): precise pulse-wave blips, echoes in time.
  bloom    warm soft chords: an FM electric piano with a slow pad underneath for swells.

One score (the "Open fifth" seed, D major pentatonic) drives all four, so the contours mean the same in every direction:
rising = here / go ahead, falling home = done, the rising pair twice = needs you, one note thinning = paused,
a falling minor third ("uh-oh") = didn't work.
"""
# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy>=2.0", "scipy>=1.13", "soundfile>=0.12", "matplotlib>=3.8"]
# ///
from __future__ import annotations

import json
import subprocess
import sys
import zlib
from dataclasses import dataclass, field, replace
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy import signal

sys.path.insert(0, str(Path(__file__).parent))
import measure  # noqa: E402

FS = 48_000
OUT = Path(__file__).parent / "out"
TAU = 2 * np.pi

# ================================================================ pitch
PENTA = [0, 2, 4, 7, 9]           # D major pentatonic: D E F# A B (any subset is consonant)
NAMES = ["D", "E", "F#", "A", "B"]


def freq(deg: float, octave: int = 0) -> float:
    """Scale degree -> Hz. Degree 0 is D5 (587.33 Hz); 5 is D6; -2 is A4; -3 is F#4."""
    o, i = divmod(int(deg), 5)
    return 587.3295 * 2 ** (o + octave + PENTA[i] / 12)


def name(deg: int, octave: int = 0) -> str:
    o, i = divmod(deg, 5)
    return f"{NAMES[i]}{5 + o + octave}"


# ================================================================ score
@dataclass
class Ev:
    t: float                      # onset (s)
    deg: int                      # scale degree (see freq)
    kind: str = "strike"          # strike | swell | tick
    vel: float = 1.0              # 0..1 loudness of this note within the sound
    bright: float = 1.0           # timbre brightness (strike hardness / filter opening)
    decay: float = 1.0            # decay-time multiplier
    atk: float = 0.06             # swell attack (s)
    hold: float = 0.03            # swell hold (s)
    rel: float = 0.25             # swell release (s)
    harm: list[int] = field(default_factory=list)   # chord tones some directions add under the note
    thin: bool = False            # swell whose brightness closes while it fades (paused)
    echo: bool = True             # Signal's in-time echo (off where a sound must stay dry)
    muted: bool = False           # damped articulation (Crystal): the glass touched by a finger, dull and short
    oct: int = 0                  # octave shift for this note, on top of the voice's register


@dataclass
class Role:
    id: str
    file: str                     # file stem per the brief's naming
    label: str
    group: str                    # signature | everyday | orb
    lufs: float                   # Standard target (reference mix, measure.loudness)
    max_len: float                # Standard length cap (s), from the brief's table
    variants: int = 1             # slight variants (summon, listening start/end, done get 3)
    meaning: str = ""
    option: int = 0               # >0: one of several candidates for the same sound, for the owner to pick
    option_name: str = ""
    about: str = ""
    source: str = ""              # a licensed recording to use instead of synthesis (see CREDITS.md)

    @property
    def base(self) -> str:
        return self.id.split("-o")[0]


ROLES = [
    Role("logo", "ferb-logo", "Sound logo", "signature", -20, 2.8, meaning="Hello, this is FERB"),
    Role("summon", "ferb-summon", "Summon", "everyday", -28, 0.40, 3, "I'm here"),
    Role("lstart", "ferb-listen-start", "Listening start", "everyday", -30, 0.50, 3, "Go ahead"),
    Role("lend", "ferb-listen-end", "Listening end", "everyday", -30, 0.40, 3, "Got it"),
    Role("done", "ferb-done", "Done", "everyday", -26, 0.70, 3, "Done, all good"),
    # Owner, 2026-11-01: the drip-off is the old flight sound (singing glass, home note and fifth) as one swell that
    # rises and falls with the orb setting off; there is no continuous flight sound any more.
    Role("drip", "ferb-orb-drip", "Orb drip-off", "orb", -33, 1.25, meaning="The orb sets off (the old flight sound)"),
    # Owner, 2026-11-01 (round 9): "One ping": one tone, A, the key's dominant (it wants to resolve home to D, so it
    # waits), bright and left to ring. Chosen over damped knocks, three Material alerts and two other one-tone takes.
    Role("needs", "ferb-needs-you", "Needs you", "everyday", -23, 1.4, 1, "Over here"),
    # Owner, 2026-11-01: FERB's notification sound (its banners and the reminders, schedules and notices in the notch)
    # is Material's "notification, high intensity" (CC BY 4.0).
    Role("notification", "ferb-notification", "Notification", "everyday", -25, 2.0, 1, "FERB posted a notification",
         source="material/notification_high-intensity.ogg"),
    Role("paused", "ferb-paused", "Paused", "everyday", -30, 0.6, 1, "Holding"),
    Role("error", "ferb-error", "Error", "everyday", -26, 0.9, 1, "That didn't work"),
    Role("hop", "ferb-orb-hop", "Orb hop", "orb", -32, 0.40, meaning="A tour step succeeded"),
    Role("melt", "ferb-orb-melt", "Orb melt-back", "orb", -30, 0.80, meaning="The orb arrives (at its target, and home)"),
]
# Candidates for a sound still being chosen: (id, file, label, group, LUFS, max length, meaning,
# [(option name, about, source or "")]). Each renders as <file>-o<n> and shows on the page with a Pick button.
# Empty: the family is decided (owner, 2026-11-01). The orb has no flight sound; drip-off plays whenever it sets off
# and melt-back whenever it arrives.
_OPTIONS: list = []
for _id, _file, _label, _group, _lufs, _len, _mean, _opts in _OPTIONS:
    for _i, (_name, _about, _src) in enumerate(_opts, 1):
        ROLES.append(Role(f"{_id}-o{_i}", f"{_file}-o{_i}", _label, _group, _lufs, _len, 1, _mean, _i, _name, _about, _src))
ROLE = {r.id: r for r in ROLES}
SUBTLE_DB = -5.0       # Subtle intensity: quieter by this much, shorter, less bright
SUBTLE_LEN = 0.8

A, B, H = -2, 1, 0     # the seed motif: A4 -> E5 (rising fifth) -> D5 (home)
F_SHARP4 = -3
B4 = -1
B3 = -6
M = dict(muted=True, oct=-1)   # Crystal's damped voice, in the motif's own register (its bright cues sit an octave up)


def score(role: str, subtle: bool) -> list[Ev]:
    """The shared score. Contour carries meaning; directions only change the material."""
    s = 0.7 if subtle else 1.0          # subtle: softer and tighter
    match role:
        case "logo":
            return [Ev(0.00, H - 5, "swell", 0.42, 0.6, atk=0.34, hold=0.10, rel=0.9, harm=[A]),
                    Ev(0.30, A, vel=0.72), Ev(0.50, B, vel=0.80),
                    Ev(0.76, H, vel=1.0, decay=2.4, harm=[A, H - 5]),
                    Ev(0.80, H + 3, "swell", 0.20, 0.8, atk=0.12, hold=0.25, rel=1.1),
                    Ev(0.86, H + 6, "tick", 0.22, 1.0)]
        case "summon":
            return [Ev(0.00, A, vel=0.62, bright=0.9 * s, decay=0.45), Ev(0.065, B, vel=0.9, bright=s, decay=0.55)]
        case "lstart":
            return [Ev(0.00, A, "swell", 0.55, 0.8 * s, atk=0.07, hold=0.01, rel=0.20),
                    Ev(0.09, B, "swell", 0.85, s, atk=0.08, hold=0.03, rel=0.26)]
        case "lend":
            return [Ev(0.00, B, "swell", 0.85, s, atk=0.035, hold=0.01, rel=0.18),
                    Ev(0.075, A, "swell", 0.6, 0.8 * s, atk=0.05, hold=0.01, rel=0.22)]
        case "done":
            return [Ev(0.00, B, vel=0.62, bright=0.9 * s, decay=0.5),
                    Ev(0.115, H, vel=1.0, bright=s, decay=0.9, harm=[A])]
        case "hop":
            return [Ev(0.00, A + 5, vel=0.7, bright=0.9, decay=0.3), Ev(0.07, H + 5, vel=0.9, bright=0.9, decay=0.35)]
        # ---- the damped voice (M: an octave below the bright cues). Owner's picks, 2026-11-01.
        case "paused":                                       # round 4 option 1, "One tap"
            return [Ev(0.00, H, vel=0.9, bright=0.8 * s, decay=1.1, **M)]
        case "error":                                        # round 4 option 1, "Uh-oh", made duller and sadder:
            # darker, slower (0.24 s), the second note longer and sinking onto B3 + F#4, an open fifth that implies
            # B minor (the key's sad relative), all in the scale.
            return [Ev(0.00, A, vel=0.85, bright=0.5 * s, decay=0.9, **M),
                    Ev(0.24, F_SHARP4, vel=0.80, bright=0.4 * s, decay=1.6, **M),
                    Ev(0.25, B3, vel=0.42, bright=0.3 * s, decay=1.8, **M)]
        case "needs":                                        # one tone (A, unresolved), bright, left to ring
            return [Ev(0.00, A, vel=1.0, bright=s, decay=1.2)]
        case "melt":
            return [Ev(0.00, B, "swell", 0.4, 0.7, atk=0.18, hold=0.0, rel=0.25),
                    Ev(0.13, H, "swell", 0.85, 0.8, atk=0.26, hold=0.06, rel=0.42),
                    Ev(0.40, H, vel=0.32, bright=0.6, decay=0.8)]
    raise KeyError(role)


# ================================================================ DSP primitives
def tvec(n: int) -> np.ndarray:
    return np.arange(n) / FS


def osc(t: np.ndarray, f: float, phase: float = 0.0, settle: float = 0.0, settle_tau: float = 0.03) -> np.ndarray:
    """Sine at f whose pitch starts `settle` (ratio) sharp and relaxes to f: the tension glide of a real strike.
    A few cents only; it adds life without reading as a bend."""
    ph = TAU * f * t + phase
    if settle:
        ph = ph + TAU * f * settle * settle_tau * (1 - np.exp(-t / settle_tau))
    return np.sin(ph)


def decay_env(t: np.ndarray, t60: float, fast_share: float = 0.6) -> np.ndarray:
    """Two-stage decay (a quick initial drop, then a longer ring), as struck objects do. t60 is the overall -60 dB time."""
    fast = np.exp(-6.91 * t / max(t60 * 0.35, 1e-4))
    slow = np.exp(-6.91 * t / max(t60, 1e-4))
    return fast_share * fast + (1 - fast_share) * slow


def attack_env(t: np.ndarray, atk: float) -> np.ndarray:
    if atk <= 0:
        return np.ones_like(t)
    a = np.clip(t / atk, 0, 1)
    return np.sin(0.5 * np.pi * a) ** 2


def ahr_env(t: np.ndarray, atk: float, hold: float, rel: float) -> np.ndarray:
    """Raised-cosine attack, hold, raised-cosine release: a swell with no hard edge anywhere."""
    e = attack_env(t, atk)
    r = np.clip((t - atk - hold) / max(rel, 1e-4), 0, 1)
    return e * np.cos(0.5 * np.pi * r) ** 2


def cents(c: float) -> float:
    return 2 ** (c / 1200)


def sos(kind: str, f: float | tuple, order: int = 2) -> np.ndarray:
    return signal.butter(order, f, btype=kind, fs=FS, output="sos")


def peaking(f0: float, gain_db: float, q: float) -> tuple[np.ndarray, np.ndarray]:
    a = 10 ** (gain_db / 40)
    w = TAU * f0 / FS
    al = np.sin(w) / (2 * q)
    b = np.array([1 + al * a, -2 * np.cos(w), 1 - al * a])
    den = np.array([1 + al / a, -2 * np.cos(w), 1 - al / a])
    return b / den[0], den / den[0]


def shelf_hi(f0: float, gain_db: float) -> tuple[np.ndarray, np.ndarray]:
    a = 10 ** (gain_db / 40)
    w = TAU * f0 / FS
    al = np.sin(w) / 2 * np.sqrt(2)
    c = np.cos(w)
    b = np.array([a * ((a + 1) + (a - 1) * c + 2 * np.sqrt(a) * al), -2 * a * ((a - 1) + (a + 1) * c),
                  a * ((a + 1) + (a - 1) * c - 2 * np.sqrt(a) * al)])
    den = np.array([(a + 1) - (a - 1) * c + 2 * np.sqrt(a) * al, 2 * ((a - 1) - (a + 1) * c),
                    (a + 1) - (a - 1) * c - 2 * np.sqrt(a) * al])
    return b / den[0], den / den[0]


def mix_into(out: np.ndarray, x: np.ndarray, at: float) -> None:
    i = int(round(at * FS))
    if i >= len(out):
        return
    n = min(len(x), len(out) - i)
    out[i:i + n] += x[:n]


# ================================================================ voices
class Voice:
    """One material. render(ev, pm) returns the note as mono samples starting at its own onset."""
    key = ""
    title = ""
    world = ""
    about = ""
    octave = 0                    # register shift for the everyday motif
    orb_octave = 0                # register shift for the orb's sounds
    room = dict(predelay=0.006, rt_low=0.45, rt_high=0.25, send=0.15, er=0.5)
    eq: list = []                 # (kind, f, gain, q)
    drive = 0.0                   # gentle saturation (0 = clean)
    harmony = False               # renders chord tones (Ev.harm)

    def __init__(self, rng: np.random.Generator):
        self.rng = rng

    def f(self, ev: Ev, pm: float, deg: int | None = None) -> float:
        return freq(ev.deg if deg is None else deg, self.octave + ev.oct) * pm

    def render(self, ev: Ev, pm: float) -> np.ndarray:
        if ev.kind == "swell":
            x = self.swell(ev, pm)
        elif ev.kind == "tick":
            x = self.tick(ev, pm)
        else:
            x = self.strike(ev, pm)
        if self.harmony and ev.harm:
            for d in ev.harm:
                h = self.render(replace(ev, deg=d, harm=[], vel=ev.vel * 0.42, bright=ev.bright * 0.7), pm)
                n = max(len(x), len(h))
                x = np.pad(x, (0, n - len(x))) + np.pad(h, (0, n - len(h)))
        return x

    def strike(self, ev: Ev, pm: float) -> np.ndarray:
        raise NotImplementedError

    def swell(self, ev: Ev, pm: float) -> np.ndarray:
        raise NotImplementedError

    def tick(self, ev: Ev, pm: float) -> np.ndarray:
        return self.strike(replace(ev, decay=ev.decay * 0.18, bright=ev.bright * 1.1), pm)

    def flight(self, pm: float, seconds: float) -> np.ndarray:
        """A seamless loop for the orb's flight. Tonal and very quiet; the app scales level and brightness by speed."""
        raise NotImplementedError


class Crystal(Voice):
    key, title, world = "crystal", "Crystal", "Apple Pay / AirPods: bright, glassy, crisp, short"
    about = ("A clear ringing note with a warm body an octave below it (the structure of the Mac's payment chime, "
             "not its sound), a bright strike that is gone within 40 ms, and a slow shimmer from two strings of the "
             "note tuned a fraction of a hertz apart. Played an octave up from the motif.")
    octave = 1
    room = dict(predelay=0.008, rt_low=0.55, rt_high=0.30, send=0.13, er=0.6)
    eq = [("peak", 3400, -2.5, 1.0), ("shelf", 9000, -2.0, 0)]
    # (ratio, amp, t60 s, twin detune Hz, brightness exponent)
    P = [(0.5, 0.40, 0.55, 0.0, 0.0), (1.0, 1.00, 1.15, 0.9, 0.0), (2.0, 0.20, 0.42, 1.6, 1.0),
         (3.98, 0.07, 0.14, 0.0, 1.6), (7.93, 0.035, 0.05, 0.0, 2.2)]

    # Muted: the same glass damped by a finger. Body and note only, ringing briefly; the glassy upper modes are
    # there for a few tens of milliseconds (so it still sounds like glass) and there is no bright strike.
    MUTED = [(0.5, 0.55, 0.30, 0.0, 0.0), (1.0, 1.00, 0.36, 0.6, 0.0), (2.0, 0.06, 0.10, 0.0, 1.0),
             (2.41, 0.10, 0.045, 0.0, 1.0), (4.31, 0.04, 0.025, 0.0, 1.5)]

    def strike(self, ev, pm):
        if ev.muted:
            return self.muted(ev, pm)
        f0 = self.f(ev, pm)
        n = int((1.4 * 1.15 * ev.decay + 0.02) * FS)
        t = tvec(n)
        x = np.zeros(n)
        for r, a, t60, det, bx in self.P:
            f = f0 * r
            if f > 15000:
                continue
            amp = a * ev.bright ** bx
            env = decay_env(t, t60 * ev.decay, 0.55 if r >= 1 else 0.3) * attack_env(t, 0.0012)
            ph = self.rng.uniform(0, TAU) if r != 1.0 else 0.0
            x += amp * env * osc(t, f, ph, settle=cents(4) - 1)
            if det:
                x += 0.45 * amp * env * osc(t, f + det, self.rng.uniform(0, TAU))
        # the strike: three bright partials that die within ~30 ms
        for fr, a in ((6150, 0.10), (7420, 0.07), (9380, 0.045)):
            x += a * ev.bright ** 1.5 * np.exp(-6.91 * t / 0.03) * attack_env(t, 0.0006) * osc(t, fr * pm, self.rng.uniform(0, TAU))
        return ev.vel * x

    def muted(self, ev, pm):
        f0 = self.f(ev, pm)
        n = int((0.45 * ev.decay + 0.05) * FS)
        t = tvec(n)
        x = np.zeros(n)
        for r, a, t60, det, bx in self.MUTED:
            amp = a * ev.bright ** bx * min(1.0, (f0 * r / 280) ** 3)   # laptop speakers can't play the body below ~250 Hz
            env = decay_env(t, t60 * ev.decay, 0.5) * attack_env(t, 0.0025)      # a softer contact than the strike
            x += amp * env * osc(t, f0 * r, self.rng.uniform(0, TAU) if r != 1.0 else 0.0)
            if det:
                x += 0.4 * amp * env * osc(t, f0 * r + det, self.rng.uniform(0, TAU))
        return ev.vel * 1.3 * x

    def swell(self, ev, pm):
        """Rubbed glass: the same partials, sustained, with no strike."""
        f0 = self.f(ev, pm)
        n = int((ev.atk + ev.hold + ev.rel + 0.01) * FS)
        t = tvec(n)
        env = ahr_env(t, ev.atk, ev.hold, ev.rel)
        x = np.zeros(n)
        for r, a, _t60, det, bx in self.P[:3]:
            amp = a * ev.bright ** (bx + 0.5)
            if ev.thin and r > 1:
                amp_t = amp * np.exp(-t / 0.08)
            else:
                amp_t = amp
            x += amp_t * osc(t, f0 * r, self.rng.uniform(0, TAU))
            x += 0.5 * amp_t * osc(t, f0 * r + (det or 0.7), self.rng.uniform(0, TAU))
        return ev.vel * 0.8 * env * x

    def flight(self, pm, seconds):
        """Singing glass: home note and fifth, held, with a slow shimmer."""
        n = int(seconds * FS)
        t = tvec(n)
        x = np.zeros(n)
        for deg, a in ((H, 1.0), (H + 3, 0.5)):
            f0 = freq(deg) * pm
            for r, ra, det in ((0.5, 0.45, 0.0), (1.0, 1.0, 0.75), (2.0, 0.18, 1.25)):
                x += a * ra * (osc(t, f0 * r) + 0.6 * osc(t, f0 * r + det, 1.0)) if det else a * ra * osc(t, f0 * r)
        return x


class Tactile(Voice):
    key, title, world = "tactile", "Tactile", "Google Pixel: warm, real small objects, friendly"
    about = ("A small ceramic-and-wood object tapped with a fingertip: a soft knock from the object's body (fixed "
             "resonances that don't change with the note), then a short, round pitched ring. Swells become a quick "
             "soft roll of taps instead of a synthetic fade-in.")
    octave = 0
    room = dict(predelay=0.003, rt_low=0.30, rt_high=0.16, send=0.11, er=0.8)
    eq = [("peak", 2800, -1.5, 1.2)]
    drive = 0.15
    # the pitched ring (ratio, amp, t60, bright exponent): near-harmonic, highs die fast (a tuned wood/ceramic bar)
    P = [(1.0, 1.0, 0.42, 0.0), (2.0, 0.26, 0.17, 0.8), (3.01, 0.10, 0.09, 1.2), (4.17, 0.05, 0.05, 1.6),
         (5.43, 0.025, 0.03, 2.0)]
    # the object's body: fixed resonances (Hz, amp, t60)
    BODY = [(540, 0.42, 0.05), (1180, 0.30, 0.035), (2350, 0.13, 0.022), (3900, 0.06, 0.012)]

    def strike(self, ev, pm):
        f0 = self.f(ev, pm)
        n = int((0.6 * ev.decay + 0.05) * FS)
        t = tvec(n)
        x = np.zeros(n)
        soft = 0.0015 / max(ev.bright, 0.3)          # fingertip contact time: softer = rounder
        for r, a, t60, bx in self.P:
            env = decay_env(t, t60 * ev.decay, 0.5) * attack_env(t, soft)
            x += a * ev.bright ** bx * env * osc(t, f0 * r, self.rng.uniform(0, TAU) if r > 1 else 0.0, settle=cents(9) - 1, settle_tau=0.015)
        for fb, a, t60 in self.BODY:
            j = cents(self.rng.uniform(-20, 20))       # a real object never knocks twice the same
            x += a * ev.bright ** (fb / 2000) * np.exp(-6.91 * t / t60) * attack_env(t, soft * 0.7) * osc(t, fb * j, self.rng.uniform(0, TAU))
        return ev.vel * x

    def swell(self, ev, pm):
        """A soft roll: taps 28 ms apart growing louder, the last one ringing."""
        hits = max(2, min(4, int(ev.atk / 0.026) + 1))
        n = int((ev.atk + ev.rel + 0.5) * FS)
        x = np.zeros(n)
        for i in range(hits):
            last = i == hits - 1
            v = (0.28 + 0.72 * (i + 1) / hits) * (1.0 if last else 0.55)
            d = ev.decay * (0.55 if last else 0.12)
            note = self.strike(replace(ev, kind="strike", vel=ev.vel * v, bright=ev.bright * (0.55 + 0.25 * i / hits), decay=d), pm)
            mix_into(x, note, i * 0.028)
        return x

    def tick(self, ev, pm):
        return self.strike(replace(ev, decay=ev.decay * 0.12, bright=ev.bright * 1.2), pm)

    def flight(self, pm, seconds):
        """A bead rolling: very soft taps in an even stream, pitched to the scale."""
        n = int(seconds * FS)
        x = np.zeros(n)
        step = 1 / 24
        degs = [H + 5, B + 5, H + 8, A + 10, H + 10]
        k = 0
        tt = 0.0
        while tt < seconds - 0.03:
            ev = Ev(0, degs[k % len(degs)], "strike", 0.35 + 0.15 * self.rng.random(), 0.5, decay=0.06)
            mix_into(x, self.strike(ev, pm)[: int(0.06 * FS)] * np.hanning(int(0.12 * FS))[int(0.06 * FS):], tt)
            tt += step * (0.85 + 0.3 * self.rng.random())
            k += 1 + int(self.rng.integers(0, 2))
        return x


class Signal(Voice):
    key, title, world = "signal", "Signal", "Nothing / Teenage Engineering: designed electronics, precise, dotted"
    about = ("A pulse wave through a filter that snaps open for a few milliseconds and closes (a clean electronic "
             "blip), with a pure tone an octave up that rings on, and one echo in time. Unapologetically electronic, "
             "but exact: no noise, no drift.")
    octave = 0
    room = dict(predelay=0.012, rt_low=0.45, rt_high=0.38, send=0.12, er=0.3)
    eq = [("peak", 3000, -2.0, 1.0)]
    ECHO = 0.135                  # one echo, in time with the motif's grid
    DUTY = 0.32

    def _pulse(self, t: np.ndarray, f0: float, fc: np.ndarray, amp: np.ndarray) -> np.ndarray:
        x = np.zeros_like(t)
        n = 1
        while f0 * n < 9000:
            a = np.sin(np.pi * n * self.DUTY) / n
            lp = 1 / np.sqrt(1 + (f0 * n / fc) ** 4)           # 2-pole low-pass response at this harmonic
            x += a * lp * np.sin(TAU * f0 * n * t)
            n += 1
        return amp * x

    def _blip(self, ev, pm, scale=1.0):
        f0 = self.f(ev, pm)
        dur = 0.32 * ev.decay + 0.04
        t = tvec(int(dur * FS))
        fc = f0 * (1.2 + 9.0 * ev.bright * np.exp(-t / 0.014))
        amp = np.exp(-6.91 * t / (0.22 * ev.decay)) * attack_env(t, 0.0008)
        x = 0.55 * self._pulse(t, f0, fc, amp)
        x += 0.42 * np.exp(-6.91 * t / (0.36 * ev.decay)) * attack_env(t, 0.0008) * osc(t, 2 * f0)
        return scale * ev.vel * x

    def strike(self, ev, pm):
        x = self._blip(ev, pm)
        if not ev.echo:
            return x
        echo = self._blip(replace(ev, bright=ev.bright * 0.45, decay=ev.decay * 0.8), pm, 0.24)
        out = np.zeros(int(self.ECHO * FS) + max(len(x), len(echo)))
        mix_into(out, x, 0)
        mix_into(out, echo, self.ECHO)
        return out

    def swell(self, ev, pm):
        """The filter opens slowly and closes again: a swell made of the same pulse."""
        f0 = self.f(ev, pm)
        n = int((ev.atk + ev.hold + ev.rel + 0.01) * FS)
        t = tvec(n)
        env = ahr_env(t, ev.atk, ev.hold, ev.rel)
        close = np.exp(-t / 0.09) if ev.thin else env
        fc = f0 * (1.1 + 2.6 * ev.bright * close)
        x = 0.5 * self._pulse(t, f0, fc, env) + 0.3 * env * osc(t, 2 * f0)
        return ev.vel * x

    def tick(self, ev, pm):
        f0 = self.f(ev, pm)
        t = tvec(int(0.06 * FS))
        return ev.vel * 0.7 * np.exp(-6.91 * t / 0.045) * attack_env(t, 0.0005) * (osc(t, 2 * f0) + 0.3 * osc(t, 3 * f0))

    def flight(self, pm, seconds):
        """A soft arpeggio of short blips in sixteenths (120 bpm), the loop a whole number of steps."""
        n = int(seconds * FS)
        x = np.zeros(n)
        degs = [H + 5, A + 5, B + 5, H + 8, B + 5, A + 5]
        steps = int(round(seconds / 0.125))
        for k in range(steps):
            ev = Ev(0, degs[k % len(degs)], vel=0.5 if k % 2 else 0.75, bright=0.35, decay=0.3)
            mix_into(x, self._blip(ev, pm), k * 0.125)
        return x


class Bloom(Voice):
    key, title, world = "bloom", "Bloom", "Warm soft chords: electric piano and slow pads"
    about = ("A soft electric piano (two-operator FM, the modulator dying away so each note starts warm-bright and "
             "turns round), voiced with chord tones underneath; swells are a slow, filtered pad. The most musical and "
             "the most gentle; the lowest register of the four.")
    octave = 0
    harmony = True
    room = dict(predelay=0.016, rt_low=1.05, rt_high=0.5, send=0.22, er=0.4)
    eq = [("peak", 3200, -1.5, 1.0)]
    drive = 0.25

    def strike(self, ev, pm):
        f0 = self.f(ev, pm)
        n = int((1.6 * ev.decay + 0.05) * FS)
        t = tvec(n)
        idx = (0.15 + 1.05 * ev.bright * ev.vel) * np.exp(-t / 0.20) + 0.12
        amp = decay_env(t, 1.45 * ev.decay, 0.5) * attack_env(t, 0.0025)
        body = np.sin(TAU * f0 * t + idx * np.sin(TAU * f0 * t))
        body2 = np.sin(TAU * (f0 + 0.9) * t + 1.3 + idx * 0.8 * np.sin(TAU * (f0 + 0.9) * t))
        bark_i = 0.55 * ev.bright * np.exp(-t / 0.011)
        bark = np.sin(TAU * f0 * t + bark_i * np.sin(TAU * 14.0 * f0 * t))
        x = amp * (0.75 * body + 0.3 * body2) + 0.18 * ev.bright * np.exp(-t / 0.015) * (bark - np.sin(TAU * f0 * t))
        return ev.vel * x

    def swell(self, ev, pm):
        """A pad: three saws a few cents apart, low-passed, the filter breathing with the level."""
        f0 = self.f(ev, pm)
        n = int((ev.atk + ev.hold + ev.rel + 0.01) * FS)
        t = tvec(n)
        env = ahr_env(t, ev.atk, ev.hold, ev.rel)
        close = np.exp(-t / 0.1) if ev.thin else env
        fc = f0 * (1.0 + 2.2 * ev.bright * close)
        x = np.zeros(n)
        for c, ph in ((-3.0, 0.3), (0.0, 1.9), (2.8, 4.1)):     # close enough not to beat within a short swell
            f = f0 * cents(c)
            k = 1
            while f * k < 6000:
                lp = 1 / np.sqrt(1 + (f * k / fc) ** 4)
                x += lp / k * np.sin(TAU * f * k * t + ph * k)
                k += 1
        return ev.vel * 0.42 * env * x

    def flight(self, pm, seconds):
        n = int(seconds * FS)
        t = tvec(n)
        x = np.zeros(n)
        for deg, a in ((H, 1.0), (H + 3, 0.6), (B + 5, 0.25)):
            f = freq(deg) * pm
            for c, ph in ((-5.0, 0.2), (5.0, 2.2)):
                k = 1
                while f * k < 4000:
                    x += a / k ** 1.6 * np.sin(TAU * f * cents(c) * k * t + ph * k)
                    k += 1
        return x


VOICES = {v.key: v for v in (Crystal, Tactile, Signal, Bloom)}


# ================================================================ room
def room_ir(p: dict, seed: int = 7) -> np.ndarray:
    """A small stereo room as an impulse response: sparse early reflections, then a diffuse tail whose highs die
    faster than its lows (per-octave decay). Delivered as its own file; the app convolves it after placing the dry."""
    rng = np.random.default_rng(seed)
    rt_lo, rt_hi = p["rt_low"], p["rt_high"]
    n = int((p["predelay"] + 1.3 * rt_lo + 0.05) * FS)
    t = tvec(n)
    ir = np.zeros((n, 2))
    pre = int(p["predelay"] * FS)
    centers = [250, 500, 1000, 2000, 4000, 8000, 14000]
    for ch in range(2):
        tail = np.zeros(n)
        for i, fc in enumerate(centers):
            rt = rt_lo * (rt_hi / rt_lo) ** (i / (len(centers) - 1))
            lo, hi = fc / np.sqrt(2), min(fc * np.sqrt(2), FS / 2 - 100)
            band = signal.sosfilt(sos("bandpass", (lo, hi), 2), rng.standard_normal(n))
            tail += band * np.exp(-6.91 * np.maximum(t - p["predelay"], 0) / rt)
        tail[:pre] = 0
        tail[pre:] *= attack_env(t[: n - pre], 0.012)
        er = np.zeros(n)
        for k, ms in enumerate(np.sort(rng.uniform(2, 24, 9))):
            i = pre + int(ms / 1000 * FS)
            er[i] += (1 if rng.random() < 0.5 else -1) * 0.9 ** k
        er = signal.sosfilt(sos("lowpass", 7000, 2), er)
        tail /= np.sqrt(np.sum(tail ** 2))
        er /= np.sqrt(np.sum(er ** 2)) or 1
        ir[:, ch] = tail + p["er"] * er
    fade = int(0.1 * n)
    ir[-fade:] *= np.cos(0.5 * np.pi * np.linspace(0, 1, fade))[:, None] ** 2
    return ir / np.sqrt(np.sum(ir ** 2) / 2)


# ================================================================ chain
def eq_chain(x: np.ndarray, v: type[Voice]) -> np.ndarray:
    y = signal.sosfilt(sos("highpass", 150, 2), x)        # nothing essential below ~200 Hz (MacBook speakers)
    for kind, f, g, q in v.eq:
        b, a = peaking(f, g, q) if kind == "peak" else shelf_hi(f, g)
        y = signal.lfilter(b, a, y)
    if v.drive:
        k = 1 + 6 * v.drive
        peak = np.max(np.abs(y)) or 1
        y = (np.tanh(k * y / peak) / np.tanh(k)) * peak * (1 - v.drive) + y * v.drive   # gentle density, unity-ish peak
    return y


def fade_tail(x: np.ndarray, cap: float, floor_db: float = -62) -> np.ndarray:
    """Trim after the sound falls below floor_db (or at cap), with a raised-cosine fade that ends on exact zero."""
    env = np.abs(x) if x.ndim == 1 else np.abs(x).max(axis=1)
    peak = env.max() or 1
    above = np.nonzero(env > peak * 10 ** (floor_db / 20))[0]
    end = min(len(x), (above[-1] + int(0.004 * FS)) if len(above) else len(x), int(cap * FS))
    y = x[:end].copy()
    f = max(int(0.015 * FS), int(0.14 * end))
    w = np.cos(0.5 * np.pi * np.linspace(0, 1, f)) ** 2
    y[-f:] = (y[-f:].T * w).T
    y[-1] = 0
    return y


def start_clean(x: np.ndarray) -> np.ndarray:
    """First sample is the first sound: a 0.3 ms raised-cosine fade-in so a non-zero start can't click."""
    n = int(0.0003 * FS)
    w = np.sin(0.5 * np.pi * np.linspace(0, 1, n)) ** 2
    x[:n] = (x[:n].T * w).T
    return x


@dataclass
class Rendered:
    dry: np.ndarray
    wet: np.ndarray
    ref: np.ndarray
    lufs: float


def load_source(rel: str, subtle: bool) -> np.ndarray:
    """A licensed recording from source/, as mono at FS, its leading silence trimmed. Subtle: a little darker
    (low-pass at 3 kHz) on top of the quieter level and shorter cap every Subtle version gets."""
    raw = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", str(Path(__file__).parent / "source" / rel), "-vn",
                          "-f", "f32le", "-ac", "1", "-ar", str(FS), "-"], capture_output=True, check=True).stdout
    x = np.frombuffer(raw, dtype="<f4").astype(float)
    start = int(np.nonzero(np.abs(x) > np.abs(x).max() * 10 ** (-50 / 20))[0][0])
    x = x[start:]
    if subtle:
        x = signal.sosfilt(sos("lowpass", 3000, 2), x)
    return x


def render_sound(v: type[Voice], role: Role, subtle: bool, var: int, ir: np.ndarray) -> Rendered:
    seed = zlib.crc32(f"{v.key}|{role.id}|{subtle}|{var}".encode())
    rng = np.random.default_rng(seed)
    vary = var > 0                     # v1 is nominal; v2.. vary pitch (±15 cents), level (±1.5 dB) and timing
    pm = cents(rng.uniform(-15, 15)) if vary else 1.0
    gain_var = rng.uniform(-1.5, 1.5) if vary else 0.0
    voice = v(rng)
    if role.group == "orb":
        voice.octave = v.orb_octave      # the orb is small: its sounds sit in the same high register in every direction
    cap = role.max_len * (SUBTLE_LEN if subtle else 1.0)
    if role.id == "flight":
        dry = loop_seamless(voice.flight(pm, 4.0 + 0.5), 4.0)
        dry = eq_chain(np.tile(dry, 2), v)[len(dry):]     # filter a loop through itself so the seam stays clean
    elif role.source:
        dry = load_source(role.source, subtle)
        dry = start_clean(fade_tail(dry, cap))
    elif role.id == "drip":
        # The flight sound under one 1.2 s speed curve: level and brightness rise as the orb sets off and settle as it
        # slows (what the round 3 grid played for "Orb flight").
        T = 1.2
        raw = voice.flight(pm, T + 0.02)
        t = tvec(len(raw))
        sp = np.where(t < T, np.sin(np.pi * np.minimum(t / T, 1)) ** 2, 0.0)
        dark = signal.sosfilt(sos("lowpass", 1500, 2), raw)
        dry = (dark * (1 - sp) + raw * sp) * sp
        dry = start_clean(fade_tail(dry, cap))
    else:
        evs = score(role.id, subtle)
        if vary:
            evs = [replace(e, t=e.t + (rng.uniform(-0.004, 0.004) if e.t else 0)) for e in evs]
        total = max(e.t for e in evs) + 2.0
        dry = np.zeros(int(total * FS))
        for e in evs:
            mix_into(dry, voice.render(e, pm), e.t)
        dry = start_clean(fade_tail(dry, cap))
    if role.id != "flight":
        dry = fade_tail(dry if role.source else eq_chain(dry, v), cap)   # a licensed recording keeps its own sound
    send = v.room["send"] * (0.6 if role.base == "error" else 1.0) * (0.8 if subtle else 1.0)
    if role.id == "flight":
        wet = np.stack([signal.fftconvolve(np.tile(dry, 2), ir[:, c])[len(dry):2 * len(dry)] for c in range(2)], axis=1) * send
        ref = np.stack([dry, dry], axis=1) + wet
    else:
        tail = 0.3 if role.id != "logo" else 0.6
        n = len(dry) + int(tail * FS)
        wet = np.stack([np.pad(signal.fftconvolve(dry, ir[:, c]), (0, n))[:n] for c in range(2)], axis=1) * send
        ref = np.pad(np.stack([dry, dry], axis=1), ((0, n - len(dry)), (0, 0))) + wet
        ref = fade_tail(ref, cap + tail, -60)
        wet = wet[: len(ref)]
        f = max(int(0.015 * FS), int(0.14 * len(ref)))
        w = np.cos(0.5 * np.pi * np.linspace(0, 1, f)) ** 2
        wet[-f:] *= w[:, None]
        wet[-1] = 0
    target = role.lufs + (SUBTLE_DB if subtle else 0.0)
    g = 10 ** ((target - measure.loudness(ref, FS)) / 20) * 10 ** (gain_var / 20)
    dry, wet, ref = dry * g, wet * g, ref * g
    return Rendered(dry, wet, ref, measure.loudness(ref, FS))


def loop_seamless(x: np.ndarray, seconds: float) -> np.ndarray:
    """Take `seconds` of x and crossfade its end into its start (equal power), so it loops without a seam."""
    n, xf = int(seconds * FS), int(0.4 * FS)
    body = x[xf:xf + n].copy()
    head = x[:xf]
    w = np.linspace(0, 1, xf)
    body[-xf:] = body[-xf:] * np.cos(0.5 * np.pi * w) + head * np.sin(0.5 * np.pi * w)
    return body


# ================================================================ output and checks
def write(path: Path, x: np.ndarray) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    sf.write(path, x, FS, subtype="PCM_24")


def check(role: Role, subtle: bool, r: Rendered) -> list[str]:
    """The brief's technical spec, per file. Returns problems (empty = passes)."""
    bad = []
    for label, x in (("dry", r.dry), ("ref", r.ref)):
        tp = measure.true_peak_db(x)
        if tp > -1.0:
            bad.append(f"{label} true peak {tp:.1f} dBTP > -1")
        e = measure.edges(x, FS)
        if role.id != "flight":
            if e["first_step"] > 0.02:
                bad.append(f"{label} first-sample step {e['first_step']:.3f}")
            if e["last"] > 1e-6:
                bad.append(f"{label} last sample {e['last']:.2e}")
            if e["lead_ms"] > (80 if role.base in ("logo", "lstart", "melt", "paused", "drip") else 8):
                bad.append(f"{label} leading quiet {e['lead_ms']:.1f} ms")
    b = measure.bands(r.ref, FS)
    if b["<200"] > 2.0:
        bad.append(f"{b['<200']:.1f}% energy below 200 Hz")
    if b[">8k"] > 3.0:
        bad.append(f"{b['>8k']:.1f}% energy above 8 kHz")
    if b["250-4k"] < 85.0:
        bad.append(f"only {b['250-4k']:.1f}% energy in 250 Hz-4 kHz")
    return bad


def render_direction(key: str, png: bool) -> dict:
    v = VOICES[key]
    ir = room_ir(v.room)
    base = OUT / key
    write(base / "ferb-room-ir.wav", ir)
    entries, problems = [], []
    for role in ROLES:
        intensities = [False, True] if role.group == "everyday" else [False]
        for subtle in intensities:
            for var in range(role.variants):
                r = render_sound(v, role, subtle, var, ir)
                stem = role.file if role.group != "everyday" else f"{role.file}-{'subtle' if subtle else 'standard'}"
                if role.variants > 1:
                    stem += f"-v{var + 1}"
                for form, x in (("dry", r.dry), ("wet", r.wet), ("ref", r.ref)):
                    write(base / form / f"{stem}.wav", x)
                for p in check(role, subtle, r):
                    problems.append(f"{stem}: {p}")
                b = measure.bands(r.ref, FS)
                entries.append({"role": role.id, "subtle": subtle, "variant": var + 1, "file": f"{stem}.wav",
                                "dur": round(len(r.ref) / FS, 3), "dry_dur": round(len(r.dry) / FS, 3),
                                "lufs": round(r.lufs, 1), "tp": round(measure.true_peak_db(r.ref), 1),
                                "band_250_4k": round(b["250-4k"], 1), "band_2_5k": round(b["2k-5k"], 1)})
                if png and var == 0:
                    measure.spectrogram_png(r.ref, FS, base / "png" / f"{stem}.png", f"{v.title} · {stem}")
    return {"key": key, "title": v.title, "world": v.world, "about": v.about, "room": v.room,
            "sounds": entries, "problems": problems}


def notes_table() -> list[dict]:
    rows = []
    for role in ROLES:
        if role.id == "flight":
            continue
        if role.source:
            rows.append({"role": role.id, "notes": [f"0.00s {role.source} (recording)"]})
            continue
        if role.id == "drip":
            rows.append({"role": "drip", "notes": ["0.00s D5+A5 swell (singing glass, 1.2 s)"]})
            continue
        o = (Crystal.orb_octave if role.group == "orb" else Crystal.octave)    # pitches as Crystal plays them
        rows.append({"role": role.id, "notes": [f"{e.t:.2f}s {name(e.deg, o + e.oct)} {'muted' if e.muted else e.kind}"
                                                for e in score(role.id, False)]})
    return rows


# ================================================================ first launch
SOURCE = Path(__file__).parent / "source" / "google-material-hero.mp4"
FIRST_LUFS = -24.0      # under the sound logo (-20) that follows it


def first_launch() -> dict:
    """FERB's first sound (owner, 2026-11-01): Google's Material Design "hero" clip (CC BY 4.0, see CREDITS.md),
    played once as FERB starts the first time, then silence until the sound logo when the notch says hello. (Extending
    it for long loads was tried and rejected: "too much sound".) Only trimmed (its 0.33 s of leading silence), faded
    to exact zero at the end and leveled. G major into the logo's D is a plagal ("amen") resolution."""
    raw = subprocess.run(["ffmpeg", "-loglevel", "error", "-i", str(SOURCE), "-vn", "-f", "f32le", "-ac", "2",
                          "-ar", str(FS), "-"], capture_output=True, check=True).stdout
    x = np.frombuffer(raw, dtype="<f4").reshape(-1, 2).astype(float)
    env = np.abs(x).max(axis=1)
    x = start_clean(x[int(np.nonzero(env > env.max() * 10 ** (-50 / 20))[0][0]):].copy())
    x = fade_tail(x, 10.0, -50)                     # its last 0.7 s are below -50 dB: inaudible, trimmed
    x *= 10 ** ((FIRST_LUFS - measure.loudness(x, FS)) / 20)
    name = "ferb-first-launch.wav"
    write(OUT / "first-launch" / name, x)
    tp = measure.true_peak_db(x)
    info = {"file": name, "dur": round(len(x) / FS, 3), "lufs": round(measure.loudness(x, FS), 1), "tp": round(tp, 1)}
    return {"files": {"clip": info}, "problems": [f"{name}: true peak {tp:.1f}"] if tp > -1 else []}


def main(argv: list[str]) -> int:
    png = "--png" in argv
    keys = [a for a in argv if not a.startswith("--")] or ["crystal"]   # round 4: the owner chose Crystal
    manifest_path = OUT / "manifest.json"
    manifest = {"directions": {}}
    bad = 0
    for k in keys:
        d = render_direction(k, png)
        manifest["directions"][k] = d
        print(f"{d['title']:8} {len(d['sounds'])} sounds", "OK" if not d["problems"] else f"{len(d['problems'])} problems")
        for p in d["problems"]:
            print("   ", p)
        bad += len(d["problems"])
    fl = first_launch()
    manifest["first_launch"] = fl
    print("First launch", "OK" if not fl["problems"] else fl["problems"],
          {k: (v["dur"], v["lufs"], v["tp"]) for k, v in fl["files"].items()})
    bad += len(fl["problems"])
    manifest["roles"] = [{"id": r.id, "base": r.base, "label": r.label, "group": r.group, "meaning": r.meaning,
                          "lufs": r.lufs, "file": r.file, "variants": r.variants, "option": r.option,
                          "option_name": r.option_name, "about": r.about} for r in ROLES]
    manifest["score"] = notes_table()
    manifest["order"] = [k for k in VOICES if k in manifest["directions"]]
    OUT.mkdir(exist_ok=True)
    manifest_path.write_text(json.dumps(manifest, indent=1))
    page = Path(__file__).parent / "index.html"        # the audition page carries the manifest inline
    html = page.read_text()
    head, rest = html.split("/* manifest: written by render.py */", 1)
    _, tail = rest.split("/* end manifest */", 1)
    page.write_text(head + "/* manifest: written by render.py */window.FERB_SOUNDS = " + json.dumps(manifest)
                    + ";/* end manifest */" + tail)
    return 1 if bad else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
