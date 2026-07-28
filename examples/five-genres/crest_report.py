"""Report loudness, peak and crest factor for every mix.

Crest factor - peak minus integrated loudness - is the number the drum bus
compressor is meant to move. Loudness and peak alone cannot show whether the
compression did anything, because a compressor followed by makeup gain leaves
both roughly where they were; what changes is the distance between them.
"""
import os
import sys

from loudness import measure

OUT_DIR = os.environ.get("QLABS_SONGS_DIR", r"C:\Wizardry\media\audio\qlabs-songs")

SONGS = ["01_jazz_ballad", "02_neo_soul", "03_drum_and_bass",
         "04_cinematic", "05_blues"]


def report(song_ids):
    print("%-20s %9s %9s %9s" % ("mix", "LUFS", "peak", "crest"))
    print("-" * 50)
    for s in song_ids:
        p = os.path.join(OUT_DIR, s + ".wav")
        if not os.path.exists(p):
            print("%-20s %9s" % (s, "missing"))
            continue
        m = measure(p)
        if m is None:
            print("%-20s %9s" % (s, "unreadable"))
            continue
        crest = m["peak_db"] - m["lufs"]
        print("%-20s %9.1f %9.1f %9.1f" % (s, m["lufs"], m["peak_db"], crest))


if __name__ == "__main__":
    report(sys.argv[1:] or SONGS)
