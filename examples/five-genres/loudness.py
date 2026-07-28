"""Integrated loudness to ITU-R BS.1770, and true-ish peak.

RMS is a poor way to balance instruments against each other because it ignores
how the ear weights frequency: a sub bass and a ride cymbal at equal RMS are
nowhere near equally loud. BS.1770 applies a shelving pre-filter and an RLB
high-pass (together "K-weighting"), then gates out the quiet passages so a
sparse part is not measured against its own silence.
"""
import wave

import numpy as np
from scipy import signal

SR = 48000


def read_wav(path):
    w = wave.open(path, "rb")
    ch, width, sr, n = w.getnchannels(), w.getsampwidth(), w.getframerate(), w.getnframes()
    raw = w.readframes(n)
    w.close()
    if width == 3:
        a = np.frombuffer(raw, dtype=np.uint8).reshape(-1, 3).astype(np.int32)
        v = a[:, 0] | (a[:, 1] << 8) | (a[:, 2] << 16)
        v = np.where(v & 0x800000, v - (1 << 24), v).astype(np.float64) / 8388608.0
    elif width == 2:
        v = np.frombuffer(raw, dtype="<i2").astype(np.float64) / 32768.0
    elif width == 4:
        v = np.frombuffer(raw, dtype="<f4").astype(np.float64)
    else:
        raise ValueError("unsupported width %d" % width)
    return v.reshape(-1, ch), sr


def k_weight(x, sr):
    """Stage 1 high-shelf + stage 2 RLB high-pass, BS.1770-4 coefficients."""
    # coefficients are specified at 48 kHz; resampling is out of scope here
    b1 = np.array([1.53512485958697, -2.69169618940638, 1.19839281085285])
    a1 = np.array([1.0, -1.69065929318241, 0.73248077421585])
    b2 = np.array([1.0, -2.0, 1.0])
    a2 = np.array([1.0, -1.99004745483398, 0.99007225036621])
    y = signal.lfilter(b1, a1, x, axis=0)
    return signal.lfilter(b2, a2, y, axis=0)


def integrated_lufs(x, sr=SR):
    """Gated integrated loudness. x is (n, channels)."""
    if x.ndim == 1:
        x = x[:, None]
    y = k_weight(x, sr)
    block = int(0.400 * sr)
    step = int(block * 0.25)          # 75 % overlap, as specified
    if len(y) < block:
        block, step = len(y), max(1, len(y) // 4)
    powers = []
    for i in range(0, max(1, len(y) - block + 1), step):
        seg = y[i:i + block]
        powers.append(float(np.mean(np.sum(seg ** 2, axis=1) / seg.shape[1])))
    powers = np.array(powers)
    powers[powers <= 0] = 1e-20
    loud = -0.691 + 10 * np.log10(powers)

    keep = loud > -70.0                       # absolute gate
    if not keep.any():
        return -70.0
    rel = -0.691 + 10 * np.log10(np.mean(powers[keep])) - 10.0   # relative gate
    keep &= loud > rel
    if not keep.any():
        return -70.0
    return float(-0.691 + 10 * np.log10(np.mean(powers[keep])))


def peak_dbfs(x):
    p = float(np.max(np.abs(x))) if len(x) else 0.0
    return 20 * np.log10(p) if p > 0 else -120.0


def measure(path):
    """Measure a render, or return None if it is missing or unreadable.

    A render that timed out leaves a truncated file behind. Raising from inside
    the measurement turns one slow render into a failed batch, so an unusable
    file is reported as absent and the caller decides what to do about it.
    """
    try:
        x, sr = read_wav(path)
    except Exception:
        return None
    if not len(x):
        return None
    return {"lufs": integrated_lufs(x, sr), "peak_db": peak_dbfs(x),
            "seconds": len(x) / float(sr)}


if __name__ == "__main__":
    import os
    import sys
    for p in sys.argv[1:]:
        m = measure(p)
        print("%-34s %7.1f LUFS   peak %6.1f dBFS   %5.1fs"
              % (os.path.basename(p), m["lufs"], m["peak_db"], m["seconds"]))
