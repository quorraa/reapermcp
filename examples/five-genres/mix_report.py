"""Measure how hard the master limiter is working, and whether the kick is there."""
import os
import wave

import numpy as np

D = os.environ.get("QLABS_SONGS_DIR",
                         os.path.join(os.path.expanduser("~"), "qlabs-songs"))
NAMES = ["01_jazz_ballad", "02_neo_soul", "03_drum_and_bass", "04_cinematic", "05_blues"]


def load(path):
    w = wave.open(path, "rb")
    ch, sr, n = w.getnchannels(), w.getframerate(), w.getnframes()
    raw = w.readframes(n)
    w.close()
    a = np.frombuffer(raw, dtype=np.uint8).reshape(-1, 3).astype(np.int32)
    v = a[:, 0] | (a[:, 1] << 8) | (a[:, 2] << 16)
    v = np.where(v & 0x800000, v - (1 << 24), v).astype(np.float64) / 8388608.0
    return v[::ch], sr


print("%-20s %6s %6s %8s %9s %9s" % ("file", "peak", "rms", "crest", "<120Hz", ">6kHz"))
for name in NAMES:
    p = os.path.join(D, name + ".wav")
    if not os.path.exists(p):
        print("%-20s MISSING" % name)
        continue
    x, sr = load(p)
    pk = float(np.max(np.abs(x)))
    rms = float(np.sqrt((x * x).mean()))
    crest = 20 * np.log10(pk / rms) if rms else 0.0
    sp = np.abs(np.fft.rfft(x * np.hanning(len(x))))
    fr = np.fft.rfftfreq(len(x), 1.0 / sr)
    tot = float(sp.sum()) or 1.0
    low = float(sp[(fr > 30) & (fr < 120)].sum()) / tot * 100
    high = float(sp[fr > 6000].sum()) / tot * 100
    # how much of the signal sits within 0.5 dB of the ceiling: limiter pinning
    pinned = float((np.abs(x) > pk * 0.944).mean()) * 100
    print("%-20s %6.3f %6.3f %7.1fdB %8.1f%% %8.1f%%   pinned=%.2f%%"
          % (name, pk, rms, crest, low, high, pinned))
