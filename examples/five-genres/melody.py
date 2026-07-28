"""Compose the example melodies instead of hand-listing their notes.

The melodies in `songs.py` were written out note by note, and it showed: nearly
every note on the beat, durations of one to four beats, a stepwise wander around
the scale, no rests shaping the line and no shape to speak of. Harmonising a dull
tune produces a well-dressed dull tune, so the worked example was under-selling
the thing it exists to demonstrate.

This builds melodies the way a melody is actually built:

* **A motif**, three to five notes with a defined interval shape and rhythm.
  Everything after it is derived from it, so the tune has an identity.
* **Development** rather than repetition - sequence at a new scale degree,
  inversion of the contour, fragmentation to the first cell, rhythmic
  augmentation. This is what makes a line feel argued rather than wandered.
* **Phrases in pairs.** The antecedent ends unresolved on 2, 5 or 7; the
  consequent answers it and lands on the tonic. Four phrases give the
  familiar 4+4+4+4 sixteen-bar shape.
* **One climax.** A single highest note in the third phrase, approached by leap
  and left by step, which is what gives a melody a peak instead of a plateau.
* **Rhythm that breathes** - syncopation off the beat, held notes across bar
  lines, and rests placed at phrase ends so the line has punctuation.

Everything is seeded, so a given song regenerates identically.
"""
import random

NOTE_NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
FLAT_NAMES = {"C#": "Db", "D#": "Eb", "F#": "Gb", "G#": "Ab", "A#": "Bb"}

SCALES = {
    "major":      [0, 2, 4, 5, 7, 9, 11],
    "minor":      [0, 2, 3, 5, 7, 8, 10],
    "dorian":     [0, 2, 3, 5, 7, 9, 10],
    "mixolydian": [0, 2, 4, 5, 7, 9, 10],
    "blues":      [0, 3, 5, 6, 7, 10],
}


def _pitch_name(midi, prefer_flats=False):
    name = NOTE_NAMES[midi % 12]
    if prefer_flats and name in FLAT_NAMES:
        name = FLAT_NAMES[name]
    return "%s%d" % (name, midi // 12 - 1)


def _degree_to_midi(root_midi, scale, degree):
    """Scale degree (0-based, may run past an octave or below zero) -> MIDI."""
    n = len(scale)
    octave, index = divmod(degree, n)
    return root_midi + 12 * octave + scale[index]


class Motif:
    """A short cell: scale-degree steps away from a starting degree, plus rhythm."""

    def __init__(self, steps, rhythm):
        self.steps = steps            # relative degrees, first is always 0
        self.rhythm = rhythm          # (offset, duration) pairs, in beats

    def at(self, start_degree, transpose=0, invert=False, stretch=1.0):
        out = []
        for (step, (off, dur)) in zip(self.steps, self.rhythm):
            s = -step if invert else step
            out.append((start_degree + transpose + s, off * stretch, dur * stretch))
        return out


# Genre shapes the cell: its interval reach and how it sits against the beat.
GENRE_MOTIFS = {
    "jazz_standard": Motif([0, 2, 1, -1], [(0.0, 0.75), (0.75, 0.75), (1.5, 0.5), (2.0, 1.5)]),
    "neo_soul_rnb":  Motif([0, 1, 3, 2],  [(0.0, 0.5), (0.75, 0.75), (1.75, 0.75), (2.5, 1.5)]),
    "drum_and_bass": Motif([0, 4, 3, 1],  [(0.0, 0.5), (0.5, 0.5), (1.5, 0.5), (2.5, 1.0)]),
    "cinematic":     Motif([0, 3, 2],     [(0.0, 2.0), (2.0, 2.0), (4.0, 4.0)]),
    "blues":         Motif([0, 2, 1, 0],  [(0.0, 1.0), (1.0, 0.5), (1.75, 0.75), (2.5, 1.5)]),
}

# How each phrase relates to the motif. The third phrase carries the climax.
PLAN = [
    dict(transpose=0,  invert=False, stretch=1.0),   # statement
    dict(transpose=2,  invert=False, stretch=1.0),   # sequence, up a third
    dict(transpose=4,  invert=False, stretch=1.0),   # climax phrase
    dict(transpose=-1, invert=True,  stretch=1.25),  # inverted, broadened answer
]

# Where a phrase comes to rest: dominant, tonic, dominant, tonic. The tonic has
# to be asserted, not merely implied. An earlier version cadenced 5-2-7-1 and
# opened away from the tonic, and the analyser read the F major ballad as G
# dorian - the same seven notes with the weight in the wrong place.
CADENCE_DEGREE = [4, 0, 4, 0]

# Held long, because duration is what makes a pitch read as the tonal centre -
# and the tonic is held longer than the dominant. Holding both for the same
# three beats left the F major ballad reading as C mixolydian by a hair: same
# seven notes, and the dominant had been given equal weight to the tonic.
CADENCE_BEATS = {0: 3.5, 4: 2.0}
DEFAULT_CADENCE_BEATS = 2.5

# Silence at the end of each phrase. Phrase detection needs somewhere to put a
# boundary; a melody that plays continuously reads as one long phrase, and a
# single phrase collapses the harmonic grid to a single slot.
PHRASE_REST_BEATS = 1.0


def compose(root_midi, mode, bars, beats_per_bar, profile, seed,
            prefer_flats=False, low=None, high=None):
    """-> [(pitch_name, start_beat, duration_beats, velocity)]"""
    rng = random.Random(seed)
    scale = SCALES[mode]
    motif = GENRE_MOTIFS.get(profile, GENRE_MOTIFS["jazz_standard"])

    phrases = 4
    phrase_bars = max(1, bars // phrases)
    phrase_beats = phrase_bars * beats_per_bar
    low = low if low is not None else root_midi - 2
    high = high if high is not None else root_midi + 16

    notes = []
    for p in range(phrases):
        shape = PLAN[p % len(PLAN)]
        base = phrase_beats * p
        # open on the tonic, and return to it or the third thereafter
        start_degree = 0 if p == 0 else rng.choice([0, 0, 2, 4])

        # A developed cell in every bar of the phrase. Stating the motif twice
        # in sixteen beats leaves the melody absent for bars at a time; the
        # argument has to continue, not pause.
        cells = []
        for bar in range(phrase_bars):
            at = bar * beats_per_bar
            if bar == phrase_bars - 1:
                continue                      # last bar belongs to the cadence
            if bar == 0:                      # statement
                cell = motif.at(start_degree, shape["transpose"],
                                shape["invert"], shape["stretch"])
            elif bar == 1:                    # sequence, one degree higher
                cell = motif.at(start_degree, shape["transpose"] + 1,
                                shape["invert"], shape["stretch"])
            else:
                # Fragment: the head of the cell only, answered a step lower.
                # Deliberately just two notes - filling every bar with a full
                # cell makes an unbroken stream, and a melody with no seams
                # reads to the analyser as a single phrase.
                cell = motif.at(start_degree, shape["transpose"] - 1,
                                not shape["invert"], shape["stretch"])[:2]
            cells.append([(d, o + at, dur) for d, o, dur in cell])

        for cell in cells:
            for degree, off, dur in cell:
                t = base + off
                if t >= base + phrase_beats - 0.5:
                    continue
                midi = _degree_to_midi(root_midi, scale, degree)
                while midi > high:
                    midi -= 12
                while midi < low:
                    midi += 12
                # Push some notes off the beat - but never the first note of a
                # phrase. Phrase boundaries score on arrival exactly on a bar
                # line, so syncopating that one note throws away the strongest
                # segmentation signal the melody has, and the whole tune comes
                # back as a single phrase.
                first_of_phrase = abs(t - base) < 1e-6
                if not first_of_phrase and rng.random() < 0.35 and dur > 0.5:
                    t += 0.25
                    dur -= 0.25
                vel = 78 + rng.randint(-6, 10)
                notes.append([midi, t, max(0.25, dur), vel])

        # The phrase comes to rest on a held note, then stops, so the line
        # punctuates and the next phrase has somewhere to begin.
        degree = CADENCE_DEGREE[p % 4]
        held = CADENCE_BEATS.get(degree, DEFAULT_CADENCE_BEATS)
        cad_at = base + phrase_beats - held - PHRASE_REST_BEATS
        cad = _degree_to_midi(root_midi, scale, degree)
        while cad > high:
            cad -= 12
        while cad < low:
            cad += 12
        notes = [n for n in notes if n[1] + n[2] <= cad_at + 0.01]
        notes.append([cad, cad_at, held, 76])

    # One climax: the highest note of the third phrase is raised so the melody
    # has a single peak, approached by leap and quit by step.
    third = [n for n in notes if phrase_beats * 2 <= n[1] < phrase_beats * 3]
    if third:
        peak = max(third, key=lambda n: n[0])
        # Up a scale step, not a fixed interval: +3 semitones invents a note
        # outside the key, which muddies exactly the reading this melody needs
        # the analyser to get right.
        step = scale[1] if len(scale) > 1 else 2
        if peak[0] + step <= high:
            peak[0] += step
        peak[3] = min(110, peak[3] + 14)

    notes.sort(key=lambda n: n[1])

    # Play it legato: every note runs to the next onset, except where a phrase
    # deliberately breathes. A melody chopped into detached fragments reads as
    # one undifferentiated phrase - the hand-written line it replaced averaged
    # 1.8 beats a note with no gaps at all, and the analyser found four phrases
    # in it against one in an earlier, choppier version of this.
    for a, b in zip(notes, notes[1:]):
        gap = b[1] - (a[1] + a[2])
        if 0 < gap < PHRASE_REST_BEATS:
            a[2] = b[1] - a[1]

    # A melody is one voice: no note may outlast the next one's start. The
    # rhythmic stretch and the legato pass above can both push a note past its
    # successor, and even a 0.125 beat overlap is enough to wreck the analysis -
    # one such overlap produced a single "phrase" 0.125 beats long, which
    # collapsed the harmonic grid to one slot and yielded exactly one chord for
    # a sixteen bar tune.
    # Round before the overlap check, not after: rounding a note's start and
    # length independently can put its end past the next onset again, and the
    # analysis is sensitive to overlaps a thousandth of a beat wide.
    for n in notes:
        n[1] = round(n[1], 3)
        n[2] = round(n[2], 3)

    kept = []
    for i, n in enumerate(notes):
        nxt = notes[i + 1][1] if i + 1 < len(notes) else None
        if nxt is not None and n[1] + n[2] > nxt:
            n[2] = nxt - n[1]
        if n[2] >= 0.125:            # anything shorter is not a note, it is a click
            kept.append(n)
    notes = kept

    return [(_pitch_name(m, prefer_flats), t, d, v) for m, t, d, v in notes]


# root, mode, whether the key is written with flats, and the register the tune
# should occupy for that instrument
SONG_KEYS = {
    "01_jazz_ballad":   dict(root_midi=65, mode="major",      prefer_flats=True,  low=60, high=81),
    "02_neo_soul":      dict(root_midi=63, mode="dorian",     prefer_flats=True,  low=58, high=79),
    "03_drum_and_bass": dict(root_midi=69, mode="minor",      prefer_flats=False, low=64, high=88),
    "04_cinematic":     dict(root_midi=62, mode="minor",      prefer_flats=True,  low=55, high=79),
    "05_blues":         dict(root_midi=64, mode="blues",      prefer_flats=False, low=59, high=83),
}


def for_song(song):
    """Compose the melody for one entry of songs.SONGS."""
    key = SONG_KEYS[song["id"]]
    return compose(bars=song["bars"], beats_per_bar=song["meter"][0],
                   profile=song["profile"], seed=song["seed"], **key)


if __name__ == "__main__":
    import songs
    for s in songs.SONGS:
        new = for_song(s)
        old = s["notes"]
        offs = sum(1 for _, t, _, _ in new if abs(t - round(t)) > 1e-6)
        span = max(n[0] for n in new), min(n[0] for n in new)
        print("%-18s %2d notes (was %2d)  off-beat %2d  durations %s"
              % (s["id"], len(new), len(old), offs,
                 sorted({d for _, _, d, _ in new})))
