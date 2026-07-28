"""Describe a render by its spectrum, so two patches can be told apart.

Loudness cannot distinguish instruments, and samples cannot be compared directly
because Surge randomises oscillator phase per render - the same project rendered
twice differs in ~90% of its samples. What stays put is the shape of the
spectrum, so that is what gets compared.
"""
import wave

import numpy as np


def load(path):
    with wave.open(path, "rb") as w:
        n, ch, sw = w.getnframes(), w.getnchannels(), w.getsampwidth()
        raw = w.readframes(n)
        sr = w.getframerate()
    if sw == 2:
        a = np.frombuffer(raw, dtype="<i2").astype(np.float64) / 32768.0
    elif sw == 3:
        b = np.frombuffer(raw, dtype=np.uint8).reshape(-1, 3).astype(np.int32)
        v = (b[:, 0] | (b[:, 1] << 8) | (b[:, 2] << 16))
        v = np.where(v & 0x800000, v - 0x1000000, v)
        a = v.astype(np.float64) / 8388608.0
    elif sw == 4:
        a = np.frombuffer(raw, dtype="<f4").astype(np.float64)
    else:
        raise ValueError("unsupported sample width %d" % sw)
    if ch > 1:
        a = a.reshape(-1, ch).mean(axis=1)
    return a, sr


def spectrum(path, bands=16):
    a, sr = load(path)
    if a.size == 0 or not np.any(a):
        return None
    win = 16384
    hop = win
    mags = []
    for i in range(0, max(1, len(a) - win), hop):
        seg = a[i:i + win] * np.hanning(win)
        mags.append(np.abs(np.fft.rfft(seg)))
    if not mags:
        return None
    m = np.mean(mags, axis=0)
    freqs = np.fft.rfftfreq(win, 1.0 / sr)

    total = m.sum() + 1e-12
    centroid = float((freqs * m).sum() / total)

    # log-spaced band energies, 40 Hz to Nyquist
    edges = np.logspace(np.log10(40.0), np.log10(sr / 2.0), bands + 1)
    idx = np.searchsorted(freqs, edges)
    energy = np.array([m[idx[k]:idx[k + 1]].sum() for k in range(bands)])
    energy = energy / (energy.sum() + 1e-12)
    return {"centroid": centroid, "bands": energy}


def distance(a, b):
    """0 means the same timbre; larger means more different."""
    if a is None or b is None:
        return float("nan")
    return float(np.abs(a["bands"] - b["bands"]).sum())


if __name__ == "__main__":
    import os
    import sys
    files = sys.argv[1:]
    specs = {}
    for f in files:
        s = spectrum(f)
        specs[f] = s
        print("%-40s centroid %8.1f Hz" % (os.path.basename(f),
                                           s["centroid"] if s else float("nan")))
    print()
    for i in range(len(files)):
        for j in range(i + 1, len(files)):
            print("%-26s vs %-26s  distance %.3f"
                  % (os.path.basename(files[i]), os.path.basename(files[j]),
                     distance(specs[files[i]], specs[files[j]])))
