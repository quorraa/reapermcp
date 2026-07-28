"""Five songs in five genres, as data.

Each melody is hand-written. The harmony, bass and countermelody are left to the
MCP server to generate from the melody, which is the point of the exercise; only
the tune, the instrument timbres and the effect chains are authored here.

Notes are (pitch, onset_in_beats, duration_in_beats, velocity). Bar N starts at
(N-1) * beats_per_bar.

ReaSynth parameters, by index, from probing the plugin:
  0 Attack   1 Release   2 Square mix   3 Saw mix   4 Triangle mix
  5 Volume   6 Decay     7 Extra sine mix          9 Sustain
  10 Pulse Width        11 Global detune          13 Portamento
Values below are normalised 0..1 and set with TrackFX_SetParamNormalized.
"""

STEP = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}


def midi(name):
    step = STEP[name[0]]
    i = 1
    while i < len(name) and name[i] in "#b":
        step += 1 if name[i] == "#" else -1
        i += 1
    return (int(name[i:]) + 1) * 12 + step


# ---------------------------------------------------------------- timbres ---
# Attack/Release/Decay are normalised; small = fast.
PAD = {0: 0.35, 1: 0.55, 2: 0.0, 3: 0.10, 4: 0.75, 7: 0.55, 9: 1.0, 6: 0.60}
KEYS = {0: 0.02, 1: 0.22, 2: 0.05, 3: 0.15, 4: 0.70, 7: 0.45, 9: 0.85, 6: 0.30}
EPIANO = {0: 0.01, 1: 0.28, 2: 0.0, 3: 0.08, 4: 0.80, 7: 0.60, 9: 0.75, 6: 0.35}
STAB = {0: 0.005, 1: 0.10, 2: 0.35, 3: 0.75, 4: 0.20, 7: 0.10, 9: 0.55, 6: 0.18}
GUITAR = {0: 0.01, 1: 0.20, 2: 0.55, 3: 0.60, 4: 0.15, 7: 0.10, 9: 0.60, 6: 0.25}
SUBBASS = {0: 0.01, 1: 0.18, 2: 0.0, 3: 0.30, 4: 0.55, 7: 0.70, 9: 0.90, 6: 0.40}
UPBASS = {0: 0.02, 1: 0.15, 2: 0.0, 3: 0.20, 4: 0.65, 7: 0.55, 9: 0.80, 6: 0.30}

SONGS = [
    {
        "id": "01_jazz_ballad",
        "title": "Jazz Ballad in F",
        "genre": "Jazz ballad",
        "tempo": 88.0,
        "meter": (4, 4),
        "bars": 16,
        "profile": "jazz_standard",
        "loop_intent": "closed_tonic",
        "seed": 1101,
        "countermelody": {"enabled": True, "density": 0.3, "role": "counterlead"},
        "notes": [
            ("A4", 0, 2, 84), ("C5", 2, 1, 78), ("D5", 3, 1, 80),
            ("C5", 4, 3, 86), ("A4", 7, 1, 76),
            ("G4", 8, 2, 80), ("Bb4", 10, 1, 78), ("D5", 11, 1, 82),
            ("C5", 12, 4, 88),
            ("F4", 16, 1, 78), ("A4", 17, 1, 80), ("C5", 18, 2, 84),
            ("Bb4", 20, 2, 82), ("G4", 22, 2, 78),
            ("A4", 24, 2, 80), ("F4", 26, 2, 76),
            ("G4", 28, 4, 84),
            ("D5", 32, 2, 88), ("C5", 34, 1, 80), ("A4", 35, 1, 78),
            ("Bb4", 36, 3, 84), ("G4", 39, 1, 78),
            ("E5", 40, 2, 90), ("D5", 42, 2, 84),
            ("C5", 44, 4, 86),
            ("A4", 48, 1, 80), ("C5", 49, 1, 84), ("F5", 50, 2, 96),
            ("E5", 52, 2, 88), ("C5", 54, 2, 82),
            ("D5", 56, 1, 82), ("C5", 57, 1, 80), ("A4", 58, 1, 78), ("G4", 59, 1, 76),
            ("F4", 60, 4, 88),
        ],
        "timbre": {"lead": KEYS, "harmony": PAD, "bass": UPBASS, "counter": KEYS},
        "fx": {
            "lead": ["VST: ReaEQ (Cockos)", "VST: ReaComp (Cockos)"],
            "harmony": ["VST: ReaEQ (Cockos)", "VST: ReaVerbate (Cockos)"],
            "bass": ["VST: ReaEQ (Cockos)", "VST: ReaComp (Cockos)"],
            "counter": ["VST: ReaEQ (Cockos)", "VST: ReaVerbate (Cockos)"],
            "master": ["VST: ReaVerbate (Cockos)", "VST: ReaLimit (Cockos)"],
        },
        "levels": {"lead": 0.50, "harmony": 0.26, "bass": 0.40, "counter": 0.20},
    },
    {
        "id": "02_neo_soul",
        "title": "Neo-Soul in E-flat",
        "genre": "Neo-soul / R&B",
        "tempo": 76.0,
        "meter": (4, 4),
        "bars": 16,
        "profile": "neo_soul_rnb",
        "loop_intent": "seamless_color",
        "seed": 1102,
        "countermelody": {"enabled": True, "density": 0.35, "role": "counterlead"},
        "notes": [
            ("Bb4", 0, 1.5, 86), ("C5", 1.5, 0.5, 74), ("Eb5", 2, 2, 90),
            ("D5", 4, 1, 82), ("C5", 5, 1, 78), ("Bb4", 6, 2, 84),
            ("G4", 8, 1.5, 80), ("Ab4", 9.5, 0.5, 72), ("Bb4", 10, 2, 82),
            ("Ab4", 12, 4, 84),
            ("Eb5", 16, 1, 88), ("D5", 17, 1, 80), ("C5", 18, 1, 78), ("Bb4", 19, 1, 76),
            ("G4", 20, 2, 80), ("Bb4", 22, 2, 82),
            ("C5", 24, 1.5, 84), ("D5", 25.5, 0.5, 74), ("Eb5", 26, 2, 88),
            ("Bb4", 28, 4, 84),
            ("F5", 32, 2, 92), ("Eb5", 34, 2, 86),
            ("D5", 36, 1, 82), ("C5", 37, 1, 78), ("Bb4", 38, 2, 84),
            ("C5", 40, 1.5, 82), ("Bb4", 41.5, 0.5, 74), ("G4", 42, 2, 80),
            ("Ab4", 44, 4, 84),
            ("Bb4", 48, 1, 82), ("C5", 49, 1, 84), ("Eb5", 50, 1, 88), ("F5", 51, 1, 92),
            ("G5", 52, 2, 98), ("Eb5", 54, 2, 88),
            ("D5", 56, 1, 82), ("C5", 57, 1, 80), ("Bb4", 58, 1, 78), ("Ab4", 59, 1, 76),
            ("Eb4", 60, 4, 90),
        ],
        "timbre": {"lead": EPIANO, "harmony": EPIANO, "bass": SUBBASS, "counter": PAD},
        "fx": {
            "lead": ["VST: ReaEQ (Cockos)", "JS: Chorus", "VST: ReaComp (Cockos)"],
            "harmony": ["VST: ReaEQ (Cockos)", "JS: Chorus", "VST: ReaVerbate (Cockos)"],
            "bass": ["VST: ReaEQ (Cockos)", "VST: ReaComp (Cockos)"],
            "counter": ["VST: ReaEQ (Cockos)", "VST: ReaDelay (Cockos)"],
            "master": ["VST: ReaComp (Cockos)", "VST: ReaLimit (Cockos)"],
        },
        "levels": {"lead": 0.46, "harmony": 0.26, "bass": 0.44, "counter": 0.18},
    },
    {
        "id": "03_drum_and_bass",
        "title": "Drum and Bass in A minor",
        "genre": "Drum and bass",
        "tempo": 174.0,
        "meter": (4, 4),
        "bars": 16,
        "profile": "drum_and_bass",
        "loop_intent": "transition_ready",
        "seed": 1103,
        "countermelody": None,
        "drums": True,
        "notes": [
            ("A4", 0, 1, 96), ("C5", 1, 1, 86), ("E5", 2, 2, 92),
            ("D5", 4, 1, 88), ("C5", 5, 1, 84), ("A4", 6, 2, 90),
            ("G4", 8, 1, 84), ("A4", 9, 1, 86), ("C5", 10, 2, 90),
            ("E5", 12, 4, 94),
            ("A4", 16, 1, 96), ("C5", 17, 1, 86), ("E5", 18, 2, 92),
            ("F5", 20, 1, 94), ("E5", 21, 1, 88), ("C5", 22, 2, 86),
            ("D5", 24, 1, 88), ("C5", 25, 1, 84), ("A4", 26, 2, 90),
            ("E4", 28, 4, 88),
            ("C5", 32, 2, 90), ("D5", 34, 2, 88),
            ("E5", 36, 2, 94), ("G5", 38, 2, 98),
            ("F5", 40, 1, 92), ("E5", 41, 1, 88), ("D5", 42, 2, 86),
            ("C5", 44, 4, 90),
            ("A4", 48, 1, 92), ("C5", 49, 1, 88), ("E5", 50, 1, 94), ("A5", 51, 1, 100),
            ("G5", 52, 2, 96), ("E5", 54, 2, 90),
            ("D5", 56, 1, 86), ("C5", 57, 1, 84), ("B4", 58, 1, 82), ("A4", 59, 1, 88),
            ("A4", 60, 4, 96),
        ],
        "timbre": {"lead": STAB, "harmony": STAB, "bass": SUBBASS},
        "fx": {
            "lead": ["VST: ReaEQ (Cockos)", "JS: Saturation", "VST: ReaComp (Cockos)"],
            "harmony": ["VST: ReaEQ (Cockos)", "JS: 4-Tap Phaser"],
            "bass": ["VST: ReaEQ (Cockos)", "JS: Distortion", "VST: ReaComp (Cockos)"],
            "drums": ["VST: ReaEQ (Cockos)", "VST: ReaComp (Cockos)"],
            "master": ["VST: ReaXcomp (Cockos)", "VST: ReaLimit (Cockos)"],
        },
        "levels": {"lead": 0.40, "harmony": 0.20, "bass": 0.46, "drums": 0.44},
    },
    {
        "id": "04_cinematic",
        "title": "Cinematic in D minor",
        "genre": "Cinematic",
        "tempo": 60.0,
        "meter": (4, 4),
        "bars": 16,
        "profile": "cinematic",
        "loop_intent": "open_dominant",
        "seed": 1104,
        "countermelody": {"enabled": True, "density": 0.2, "role": "counterlead"},
        "notes": [
            ("D4", 0, 4, 70), ("F4", 4, 4, 74),
            ("A4", 8, 2, 78), ("G4", 10, 2, 74),
            ("F4", 12, 4, 76),
            ("E4", 16, 4, 72), ("G4", 20, 4, 76),
            ("Bb4", 24, 2, 82), ("A4", 26, 2, 78),
            ("G4", 28, 4, 76),
            ("A4", 32, 4, 80), ("C5", 36, 4, 84),
            ("D5", 40, 2, 88), ("C5", 42, 2, 82),
            ("Bb4", 44, 4, 80),
            ("A4", 48, 2, 84), ("D5", 50, 2, 90),
            ("F5", 52, 4, 100),
            ("E5", 56, 2, 90), ("D5", 58, 2, 84),
            ("D4", 60, 4, 78),
        ],
        "timbre": {"lead": PAD, "harmony": PAD, "bass": SUBBASS, "counter": PAD},
        "fx": {
            "lead": ["VST: ReaEQ (Cockos)", "VST: ReaVerbate (Cockos)"],
            "harmony": ["VST: ReaEQ (Cockos)", "VST: ReaVerbate (Cockos)"],
            "bass": ["VST: ReaEQ (Cockos)"],
            "counter": ["VST: ReaEQ (Cockos)", "VST: ReaDelay (Cockos)", "VST: ReaVerbate (Cockos)"],
            "master": ["VST: ReaVerbate (Cockos)", "VST: ReaLimit (Cockos)"],
        },
        "levels": {"lead": 0.44, "harmony": 0.30, "bass": 0.38, "counter": 0.22},
    },
    {
        "id": "05_blues",
        "title": "Twelve-Bar Blues in E",
        "genre": "Blues",
        "tempo": 100.0,
        "meter": (4, 4),
        "bars": 12,
        "profile": "blues",
        "loop_intent": "closed_tonic",
        "seed": 1105,
        "countermelody": None,
        "notes": [
            ("E4", 0, 1, 92), ("G4", 1, 1, 86), ("A4", 2, 1, 88), ("Bb4", 3, 1, 90),
            ("B4", 4, 2, 94), ("A4", 6, 1, 86), ("G4", 7, 1, 84),
            ("E4", 8, 4, 90),
            ("G4", 12, 1, 86), ("E4", 13, 1, 84), ("D4", 14, 2, 82),
            ("A4", 16, 1, 90), ("C5", 17, 1, 88), ("D5", 18, 1, 92), ("Eb5", 19, 1, 94),
            ("E5", 20, 2, 100), ("D5", 22, 2, 90),
            ("B4", 24, 2, 88), ("G4", 26, 2, 84),
            ("E4", 28, 4, 88),
            ("B4", 32, 1, 90), ("D5", 33, 1, 92), ("E5", 34, 2, 98),
            ("A4", 36, 1, 88), ("C5", 37, 1, 90), ("D5", 38, 2, 92),
            ("E4", 40, 1, 86), ("G4", 41, 1, 88), ("B4", 42, 1, 90), ("D5", 43, 1, 92),
            ("E4", 44, 4, 96),
        ],
        "timbre": {"lead": GUITAR, "harmony": KEYS, "bass": UPBASS},
        "fx": {
            "lead": ["VST: ReaEQ (Cockos)", "JS: Distortion", "VST: ReaComp (Cockos)"],
            "harmony": ["VST: ReaEQ (Cockos)", "VST: ReaComp (Cockos)"],
            "bass": ["VST: ReaEQ (Cockos)", "VST: ReaComp (Cockos)"],
            "master": ["VST: ReaComp (Cockos)", "VST: ReaLimit (Cockos)"],
        },
        "levels": {"lead": 0.42, "harmony": 0.26, "bass": 0.42},
    },
]


def validate():
    for s in SONGS:
        beats = s["bars"] * s["meter"][0]
        end = max(o + d for _, o, d, _ in s["notes"])
        assert end <= beats, "%s: notes run past the last bar (%s > %s)" % (s["id"], end, beats)
        for p, o, d, v in s["notes"]:
            midi(p)
            assert d > 0 and 1 <= v <= 127, "%s: bad note %s" % (s["id"], p)
    return True


if __name__ == "__main__":
    validate()
    for s in SONGS:
        beats = s["bars"] * s["meter"][0]
        print("%-18s %-16s %3.0f bpm  %d/%d  %2d bars  %2d notes  profile=%s" % (
            s["id"], s["genre"], s["tempo"], s["meter"][0], s["meter"][1],
            s["bars"], len(s["notes"]), s["profile"]))


# The melodies are composed rather than listed. Hand-written note lists gave
# every song the same plod: on-beat quarters and halves stepping around a
# scale, no motif, no phrase shape, no rests. Harmonising that produces a
# well-dressed dull tune, which under-sells the engine the example exists to
# show. See melody.py for how the lines are built.
import melody  # noqa: E402  (imported late: melody reads a song dict)

for _song in SONGS:
    _song["notes"] = melody.for_song(_song)
