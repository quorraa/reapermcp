"""Look for the fingerprints of crackle in a rendered WAV: clipping and
sample-to-sample discontinuities (clicks). Pure stdlib."""
import struct
import sys
import os
import wave

path = sys.argv[1]
w = wave.open(path, "rb")
ch, width, rate, n = w.getnchannels(), w.getsampwidth(), w.getframerate(), w.getnframes()
print("channels=%d width=%d bytes rate=%d frames=%d (%.2f s)" % (ch, width, rate, n, n / rate))
raw = w.readframes(n)
w.close()

if width == 2:
    samples = struct.unpack("<%dh" % (len(raw) // 2), raw)
    scale = 32768.0
elif width == 3:
    samples = []
    for i in range(0, len(raw), 3):
        v = raw[i] | (raw[i + 1] << 8) | (raw[i + 2] << 16)
        if v & 0x800000:
            v -= 1 << 24
        samples.append(v)
    scale = 8388608.0
elif width == 4:
    samples = struct.unpack("<%df" % (len(raw) // 4), raw)
    scale = 1.0
else:
    raise SystemExit("unsupported width %d" % width)

left = [samples[i] / scale for i in range(0, len(samples), ch)]

peak = max(abs(v) for v in left)
clipped = sum(1 for v in left if abs(v) >= 0.999)
# A click is a jump far larger than anything a 48 kHz audio signal should make.
jumps = []
for i in range(1, len(left)):
    d = abs(left[i] - left[i - 1])
    if d > 0.25:
        jumps.append((i / rate, d))

print("peak            = %.4f" % peak)
print("clipped samples = %d" % clipped)
print("discontinuities > 0.25 = %d" % len(jumps))
for t, d in jumps[:10]:
    print("   at %.3f s  jump %.3f" % (t, d))

# Digital silence in the middle of the piece would indicate dropouts.
run = best = 0
for v in left:
    run = run + 1 if abs(v) < 1e-6 else 0
    best = max(best, run)
print("longest silent run = %d samples (%.1f ms)" % (best, best * 1000.0 / rate))
print("VERDICT:", "CLEAN" if (clipped == 0 and len(jumps) == 0) else "ARTIFACTS PRESENT")
