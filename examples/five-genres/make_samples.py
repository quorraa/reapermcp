"""Synthesise a small sample library, because this machine has no instrument
plugins beyond ReaSynth and an empty sampler.

Everything is written as 48 kHz 24-bit mono WAV and loaded into
ReaSamplOmatic5000. Melodic samples are rendered at a known pitch so the sampler
can transpose them; the root note is in the filename and consumed by the loader.

The goal is a believable instrument, not a commercial library: a struck-string
model for the electric piano, additive+noise for the drums, and a mildly
detuned multi-oscillator stack for the pads and leads.
"""
import math
import os

import numpy as np
from scipy import signal

SR = 48000
OUT = os.path.join(
    os.environ.get("QLABS_SONGS_DIR",
                   os.path.join(os.path.expanduser("~"), "qlabs-songs")),
    "samples")


def write(name, x, sr=SR):
    os.makedirs(OUT, exist_ok=True)
    x = np.asarray(x, dtype=np.float64)
    peak = float(np.max(np.abs(x))) or 1.0
    x = (x / peak) * 0.89
    # short fade-out so no sample ends on a discontinuity
    f = min(256, len(x))
    x[-f:] *= np.linspace(1.0, 0.0, f)
    d = np.clip(np.round(x * 8388607.0), -8388608, 8388607).astype(np.int32)
    raw = bytearray()
    for v in d:
        v = int(v) & 0xFFFFFF
        raw += bytes((v & 0xFF, (v >> 8) & 0xFF, (v >> 16) & 0xFF))
    import wave
    path = os.path.join(OUT, name)
    w = wave.open(path, "wb")
    w.setnchannels(1)
    w.setsampwidth(3)
    w.setframerate(sr)
    w.writeframes(bytes(raw))
    w.close()
    return path


def env(n, a, d, s, r, sus_level=0.7):
    """Simple ADSR over n samples, times in seconds."""
    a, d, r = max(int(a * SR), 1), max(int(d * SR), 1), max(int(r * SR), 1)
    s = max(n - a - d - r, 0)
    e = np.concatenate([
        np.linspace(0, 1, a),
        np.linspace(1, sus_level, d),
        np.full(s, sus_level),
        np.linspace(sus_level, 0, r),
    ])
    return e[:n] if len(e) >= n else np.pad(e, (0, n - len(e)))


def noise(n):
    return np.random.default_rng(7).standard_normal(n)


# ------------------------------------------------------------------ drums ---
def kick(dur=0.55):
    n = int(dur * SR)
    t = np.arange(n) / SR
    # pitch sweep 110 -> 44 Hz, the classic drum-synth kick
    f = 44 + (110 - 44) * np.exp(-t * 34)
    ph = 2 * np.pi * np.cumsum(f) / SR
    body = np.sin(ph) * np.exp(-t * 6.5)
    click = noise(n) * np.exp(-t * 420) * 0.35
    x = body + click
    return np.tanh(x * 1.6)


def snare(dur=0.38):
    n = int(dur * SR)
    t = np.arange(n) / SR
    tone = (np.sin(2 * np.pi * 190 * t) + np.sin(2 * np.pi * 278 * t)) * np.exp(-t * 26) * 0.5
    nz = noise(n)
    b, a = signal.butter(2, [1200 / (SR / 2), 9000 / (SR / 2)], btype="band")
    body = signal.lfilter(b, a, nz) * np.exp(-t * 15)
    return np.tanh((tone + body) * 1.3)


def hat(dur=0.09, open_=False):
    d = 0.42 if open_ else dur
    n = int(d * SR)
    t = np.arange(n) / SR
    nz = noise(n)
    b, a = signal.butter(4, 7000 / (SR / 2), btype="high")
    x = signal.lfilter(b, a, nz)
    return x * np.exp(-t * (9 if open_ else 55))


def clap(dur=0.34):
    n = int(dur * SR)
    t = np.arange(n) / SR
    nz = noise(n)
    b, a = signal.butter(2, [1100 / (SR / 2), 6000 / (SR / 2)], btype="band")
    x = signal.lfilter(b, a, nz)
    e = np.zeros(n)
    for off, amp in ((0.000, 1.0), (0.010, 0.85), (0.020, 0.7), (0.032, 0.55)):
        i = int(off * SR)
        e[i:] += amp * np.exp(-(t[: n - i]) * 34)
    return x * e


def ride(dur=1.1):
    n = int(dur * SR)
    t = np.arange(n) / SR
    x = np.zeros(n)
    for f in (523, 731, 941, 1187, 1523, 2100):
        x += np.sin(2 * np.pi * f * t) * np.exp(-t * 3.0)
    nz = noise(n)
    b, a = signal.butter(4, 6000 / (SR / 2), btype="high")
    x = x * 0.5 + signal.lfilter(b, a, nz) * np.exp(-t * 6) * 0.5
    return x


# --------------------------------------------------------------- melodic ---
ROOT = 60           # everything melodic is rendered at middle C
F0 = 261.6255653


def epiano(dur=2.6):
    """FM: a tine struck by a hammer. Classic 2-operator electric piano."""
    n = int(dur * SR)
    t = np.arange(n) / SR
    mod_env = np.exp(-t * 9.0)
    car_env = np.exp(-t * 1.7)
    mod = np.sin(2 * np.pi * F0 * 14 * t) * mod_env * 5.2
    x = np.sin(2 * np.pi * F0 * t + mod) * car_env
    x += np.sin(2 * np.pi * F0 * 2 * t) * np.exp(-t * 3.2) * 0.12
    return x


def pluck(dur=2.2):
    """Karplus-Strong plucked string."""
    n = int(dur * SR)
    N = int(SR / F0)
    buf = list(np.random.default_rng(3).uniform(-1, 1, N))
    out = np.empty(n)
    for i in range(n):
        v = buf[0]
        out[i] = v
        buf.append(0.5 * (v + buf[1]) * 0.998)
        buf.pop(0)
    return out * np.exp(-np.arange(n) / SR * 1.1)


def pad(dur=3.4):
    """Detuned saw stack through a slow low-pass: a warm string pad."""
    n = int(dur * SR)
    t = np.arange(n) / SR
    x = np.zeros(n)
    for cents in (-9, -4, 0, 5, 11):
        f = F0 * (2 ** (cents / 1200.0))
        x += signal.sawtooth(2 * np.pi * f * t) / 5.0
    cutoff = np.linspace(600, 3200, n) / (SR / 2)
    b, a = signal.butter(2, float(np.mean(cutoff)), btype="low")
    x = signal.lfilter(b, a, x)
    return x * env(n, 0.42, 0.5, 0.75, 0.9, 0.72)


def brass(dur=2.2):
    """Saw + square with a fast filter sweep: a reedy lead."""
    n = int(dur * SR)
    t = np.arange(n) / SR
    x = signal.sawtooth(2 * np.pi * F0 * t) * 0.7 + signal.square(2 * np.pi * F0 * t) * 0.3
    b, a = signal.butter(3, 2600 / (SR / 2), btype="low")
    x = signal.lfilter(b, a, x)
    return np.tanh(x * 1.4) * env(n, 0.03, 0.25, 0.7, 0.45, 0.72)


def upbass(dur=2.4):
    """Round, woody bass: sine fundamental, a little second harmonic, fast decay."""
    n = int(dur * SR)
    t = np.arange(n) / SR
    f = F0 / 2
    x = np.sin(2 * np.pi * f * t)
    x += np.sin(2 * np.pi * f * 2 * t) * 0.22
    x += np.sin(2 * np.pi * f * 3 * t) * 0.07
    click = noise(n) * np.exp(-t * 240) * 0.08
    return (x * np.exp(-t * 1.5) + click) * env(n, 0.006, 0.3, 0.6, 0.5, 0.6)


def subbass(dur=2.2):
    """Sine sub with a touch of drive: electronic bass."""
    n = int(dur * SR)
    t = np.arange(n) / SR
    f = F0 / 2
    x = np.sin(2 * np.pi * f * t) + 0.3 * np.sin(2 * np.pi * f * 2 * t)
    return np.tanh(x * 1.8) * env(n, 0.004, 0.2, 0.85, 0.35, 0.85)


DRUMS = {"kick": kick, "snare": snare, "hat_closed": lambda: hat(),
         "hat_open": lambda: hat(open_=True), "clap": clap, "ride": ride}
MELODIC = {"epiano": epiano, "pluck": pluck, "pad": pad,
           "brass": brass, "upbass": upbass, "subbass": subbass}

if __name__ == "__main__":
    made = []
    for name, fn in DRUMS.items():
        made.append((write("drum_%s.wav" % name, fn()), "drum"))
    for name, fn in MELODIC.items():
        made.append((write("inst_%s_root%d.wav" % (name, ROOT), fn()), "melodic"))
    for path, kind in made:
        import wave
        w = wave.open(path, "rb")
        print("%-8s %-46s %5.2f s" % (kind, os.path.basename(path),
                                      w.getnframes() / float(w.getframerate())))
        w.close()
    print("\nwritten to", OUT)
