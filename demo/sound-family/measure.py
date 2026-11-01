"""Objective measurements for FERB's sound family (loudness, true peak, spectrum, edits).

Used by render.py to level and check every file, and on its own to measure reference sounds:
    uv run demo/sound-family/measure.py FILE.wav [...] [--png DIR]
"""
# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy>=2.0", "scipy>=1.13", "soundfile>=0.12", "matplotlib>=3.8"]
# ///
from __future__ import annotations

import sys
from pathlib import Path

import numpy as np
from scipy import signal


def _k_weight(x: np.ndarray, fs: int) -> np.ndarray:
    """ITU-R BS.1770 K-weighting (pre-filter shelf + RLB high-pass), coefficients derived for any rate."""
    # Stage 1: high shelf (+4 dB above ~1.5 kHz).
    f0, g, q = 1681.974450955533, 3.999843853973347, 0.7071752369554196
    k = np.tan(np.pi * f0 / fs)
    vh, vb = 10 ** (g / 20), 10 ** (g / 20) ** 0.4996667741545416
    a0 = 1 + k / q + k * k
    b1 = [(vh + vb * k / q + k * k) / a0, 2 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0]
    a1 = [1, 2 * (k * k - 1) / a0, (1 - k / q + k * k) / a0]
    # Stage 2: high-pass (~38 Hz).
    f0, q = 38.13547087602444, 0.5003270373238773
    k = np.tan(np.pi * f0 / fs)
    a2 = [1, 2 * (k * k - 1) / (1 + k / q + k * k), (1 - k / q + k * k) / (1 + k / q + k * k)]
    b2 = [1, -2, 1]
    y = signal.lfilter(b1, a1, x, axis=0)
    return signal.lfilter(b2, a2, y, axis=0)


def loudness(x: np.ndarray, fs: int, block: float = 0.1) -> float:
    """Gated integrated loudness in LUFS, BS.1770 style but with short blocks (default 100 ms, 75 % overlap)
    so sub-second UI sounds are measured over their sounding part. Stereo channels sum in power."""
    x = np.atleast_2d(x.T).T if x.ndim == 1 else x
    y = _k_weight(x, fs)
    n = max(1, int(round(block * fs)))
    hop = max(1, n // 4)
    if len(y) < n:
        y = np.concatenate([y, np.zeros((n - len(y), y.shape[1]))])
    starts = range(0, len(y) - n + 1, hop)
    z = np.array([np.sum(np.mean(y[s:s + n] ** 2, axis=0)) for s in starts])
    lk = -0.691 + 10 * np.log10(np.maximum(z, 1e-20))
    keep = lk > -70
    if not keep.any():
        return -120.0
    rel = -0.691 + 10 * np.log10(np.mean(z[keep])) - 10
    keep &= lk > rel
    return float(-0.691 + 10 * np.log10(np.mean(z[keep])))


def true_peak_db(x: np.ndarray) -> float:
    up = signal.resample_poly(x, 4, 1, axis=0)
    return float(20 * np.log10(max(np.max(np.abs(up)), 1e-12)))


def bands(x: np.ndarray, fs: int) -> dict[str, float]:
    """Share of energy (%) per band; the brief's ranges: meaning 250 Hz-4 kHz, decoration above 8 kHz."""
    m = x if x.ndim == 1 else x.mean(axis=1)
    spec = np.abs(np.fft.rfft(m)) ** 2
    f = np.fft.rfftfreq(len(m), 1 / fs)
    tot = spec.sum() or 1.0
    edges = {"<200": (0, 200), "200-250": (200, 250), "250-4k": (250, 4000), "2k-5k": (2000, 5000),
             "4k-8k": (4000, 8000), ">8k": (8000, fs / 2)}
    return {k: float(100 * spec[(f >= lo) & (f < hi)].sum() / tot) for k, (lo, hi) in edges.items()}


def centroid_track(x: np.ndarray, fs: int, hop: float = 0.025) -> list[tuple[float, float]]:
    m = x if x.ndim == 1 else x.mean(axis=1)
    n = int(0.04 * fs)
    out = []
    for s in range(0, max(1, len(m) - n), int(hop * fs)):
        seg = m[s:s + n] * np.hanning(n)
        sp = np.abs(np.fft.rfft(seg))
        f = np.fft.rfftfreq(n, 1 / fs)
        e = sp.sum()
        if e > 1e-6:
            out.append((s / fs, float((sp * f).sum() / e)))
    return out


def edges(x: np.ndarray, fs: int) -> dict[str, float]:
    """Edit checks: leading silence, first-sample level and first step, last-sample level."""
    m = np.abs(x if x.ndim == 1 else x.max(axis=1))
    peak = m.max() or 1.0
    above = np.nonzero(m > peak * 10 ** (-40 / 20))[0]
    lead = above[0] / fs if len(above) else 0.0
    d = np.abs(np.diff(x[:16], axis=0)).max() if len(x) > 16 else 0.0
    return {"lead_ms": 1000 * lead, "first": float(np.abs(x[0]).max()), "first_step": float(d),
            "last": float(np.abs(x[-1]).max()), "tail_db": float(20 * np.log10(np.abs(x[-int(0.005 * fs):]).max() / peak + 1e-12))}


def summary(x: np.ndarray, fs: int) -> dict:
    b = bands(x, fs)
    ct = centroid_track(x, fs)
    return {"dur": len(x) / fs, "lufs": loudness(x, fs), "tp": true_peak_db(x), **{f"b{k}": v for k, v in b.items()},
            "cent0": ct[0][1] if ct else 0.0, "cent300": next((c for t, c in ct if t >= 0.3), ct[-1][1] if ct else 0.0),
            **edges(x, fs)}


def spectrogram_png(x: np.ndarray, fs: int, path: Path, title: str) -> None:
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    m = x if x.ndim == 1 else x.mean(axis=1)
    f, t, s = signal.spectrogram(m, fs, nperseg=1024, noverlap=896, window="hann")
    fig, ax = plt.subplots(2, 1, figsize=(7, 4.2), gridspec_kw={"height_ratios": [1, 3]}, sharex=True)
    ax[0].plot(np.arange(len(m)) / fs, m, lw=0.4, color="k")
    ax[0].set_title(title, fontsize=9)
    ax[1].pcolormesh(t, f, 10 * np.log10(s + 1e-14), shading="auto", vmin=10 * np.log10(s.max() + 1e-14) - 80, cmap="magma")
    ax[1].set_ylim(0, 12000)
    ax[1].set_ylabel("Hz")
    fig.tight_layout()
    path.parent.mkdir(parents=True, exist_ok=True)
    fig.savefig(path, dpi=110)
    plt.close(fig)


if __name__ == "__main__":
    import soundfile as sf
    args = sys.argv[1:]
    png = None
    if "--png" in args:
        i = args.index("--png")
        png = Path(args[i + 1])
        png.mkdir(parents=True, exist_ok=True)
        del args[i:i + 2]
    print(f"{'file':28} {'dur':>5} {'LUFS':>6} {'TP':>5} {'<200':>5} {'250-4k':>6} {'2-5k':>5} {'>8k':>5} {'c0':>6} {'c300':>6} {'lead':>5}")
    for p in args:
        x, fs = sf.read(p, always_2d=False)
        s = summary(x, fs)
        print(f"{Path(p).stem[:28]:28} {s['dur']:5.2f} {s['lufs']:6.1f} {s['tp']:5.1f} {s['b<200']:5.1f} {s['b250-4k']:6.1f} "
              f"{s['b2k-5k']:5.1f} {s['b>8k']:5.1f} {s['cent0']:6.0f} {s['cent300']:6.0f} {s['lead_ms']:5.1f}")
        if png:
            spectrogram_png(x, fs, png / f"{Path(p).stem}.png", Path(p).stem)
