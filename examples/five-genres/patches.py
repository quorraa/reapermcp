"""Real Surge XT factory patches per track, instead of hand-written parameters.

The instruments were previously built by writing Surge parameters directly: one
oscillator, no unison, no layering, no filter movement. That produces a tone,
not an instrument, which is why the melodies had nothing driving them.

Surge was also running with no factory data at all - the plugin binaries had
been taken out of the portable-install zip and the SurgeXTData folder left
behind, so there were no patches and no wavetables to load.
"""
import os

DATA = r"C:\ProgramData\Surge XT\patches_factory"

# category / patch name, chosen to suit each genre and each part's job
ASSIGN = {
    "01_jazz_ballad": {
        "Melody":             ("Keys",   "Soft Suitcase"),
        "QLabsHarmony":       ("Keys",   "EP 1"),
        "QLabsBass":          ("Basses", "Fingered"),
        "QLabsCountermelody": ("Winds",  "Clarinet"),
    },
    "02_neo_soul": {
        "Melody":             ("Keys",   "DX EP"),
        "QLabsHarmony":       ("Pads",   "MKS-70 Warm Pad"),
        "QLabsBass":          ("Basses", "Rubber Bass"),
        "QLabsCountermelody": ("Plucks", "Nice Pluck 2"),
    },
    "03_drum_and_bass": {
        "Melody":             ("Leads",  "Sync Lead"),
        "QLabsHarmony":       ("Pads",   "Super"),
        "QLabsBass":          ("Basses", "Wide Bassline"),
        "QLabsCountermelody": ("Plucks", "FM Pluck"),
    },
    "04_cinematic": {
        "Melody":             ("Brass",  "JX-10 Double Brass"),
        "QLabsHarmony":       ("Pads",   "Verb Pad"),
        "QLabsBass":          ("Basses", "Sub 2"),
        "QLabsCountermelody": ("Winds",  "Flute 1"),
    },
    "05_blues": {
        "Melody":             ("Keys",   "House Organ"),
        "QLabsHarmony":       ("Keys",   "Organ 3"),
        "QLabsBass":          ("Basses", "E-Bass"),
        "QLabsCountermelody": ("Plucks", "E-Guitar"),
    },
}


def path_for(category, name):
    p = os.path.join(DATA, category, name + ".fxp")
    if not os.path.exists(p):
        raise FileNotFoundError(p)
    return p


def check_all():
    missing = []
    for song, tracks in ASSIGN.items():
        for track, (cat, name) in tracks.items():
            try:
                path_for(cat, name)
            except FileNotFoundError as e:
                missing.append("%s/%s -> %s" % (song, track, e))
    return missing


if __name__ == "__main__":
    bad = check_all()
    if bad:
        for b in bad:
            print("MISSING", b)
    else:
        n = sum(len(v) for v in ASSIGN.values())
        print("all %d patch files present" % n)
