# The musical intermediate representation

The strongly typed musical domain model that every part of this product shares.
It lives in the `music-domain` crate and is the vocabulary the analysis engine,
the harmony solver, the arrangement engine, the loop engine and the MCP surface
all speak.

This document describes what is actually implemented. Where the design has a
deliberate limit — a field that exists for future compatibility, a concept
carried as a field rather than as its own type — it says so.

Two principles run through the whole model:

1. **Musical meaning is never reduced to a number.** A chord is not a pitch-class
   set. A pitch is not a MIDI number. Time is not a float.
2. **Every type has a JSON form.** Every type here exposes
   `to_json(&self) -> qjson::Json`; the ones that arrive over IPC also expose
   `from_json`. That is what makes analyses, candidates and traces inspectable
   from an MCP client.

---

## Contents

1. [Time](#1-time)
2. [Pitch](#2-pitch)
3. [Intervals](#3-intervals)
4. [Scales](#4-scales)
5. [Notes and voices](#5-notes-and-voices)
6. [Chords](#6-chords)
7. [Musical structures](#7-musical-structures)
8. [Candidates, plans and traces](#8-candidates-plans-and-traces)
9. [Identifiers and errors](#9-identifiers-and-errors)
10. [Where each brief concept lives](#10-where-each-brief-concept-lives)

---

## 1. Time

**Quarter notes are the canonical timeline.** Not seconds, not PPQ, not bars.
Seconds are derived when needed; PPQ is retained only as provenance.

### `BeatTime` — exact musical position and duration

```rust
pub struct BeatTime { num: i64, den: i64 }   // invariant: den > 0, gcd(|num|, den) == 1
```

A **normalized rational number of quarter notes**. Always reduced, always with a
positive denominator, so equality and ordering are exact. This is the single
most consequential type in the model: loop-boundary equality, bar-grid
alignment and round-trip provenance all require exact comparison, and a float
cannot provide it. See decision D3 in [`ARCHITECTURE.md`](ARCHITECTURE.md).

- `BeatTime::ZERO`, `new(num, den)` (panics only on a zero denominator),
  `try_new`, `from_quarters(i64)`.
- `from_f64(q)` snaps a float onto a **1/1920-quarter grid** (`GRID_DEN = 1920`,
  which is `2^7 × 3 × 5`), representing 128th notes, triplets and quintuplets
  exactly. `from_f64_grid(q, den)` lets you pick another grid.
  Magnitudes beyond `MAX_QUARTERS` saturate rather than overflow.
- `as_f64`, `num`, `den`, `is_zero`, `is_positive`, `min`, `max`, `abs`.
- `rem_euclid(m)`, `div_floor(m)`, `scale(num, den)`.
- `to_display()` renders `"3"`, `"1/2"`, `"7/3"` — the form fixtures and JSON use.
- Implements `Add`, `Sub`, `Neg`, `Mul<i64>`, `Ord`, `PartialOrd`, `Display`.

`PPQ = 960` is exported as a reference constant for REAPER PPQ provenance only.
Nothing in the engine computes with it.

### `TimeSignature`

```rust
pub struct TimeSignature { pub numerator: u16, pub denominator: u16 }
```

- `bar_length_qn()` → `4·n/d` as a `BeatTime`; `beat_unit_qn()` → `4/d`.
- `is_compound()` — true when `n % 3 == 0 && n > 3 && d >= 8`.
- `metric_weight(offset_in_bar)` → `0.0..=1.0`, downbeat `1.0`. This is what
  salience and grid selection consult; metric strength is data, not a
  hardcoded "beats 1 and 3" assumption.
- `parse("4/4")`, `to_display()`.

### Tempo and meter events

```rust
pub struct TempoEvent { pub qn: BeatTime, pub bpm: f64, pub linear: bool }
pub struct MeterEvent { pub qn: BeatTime, pub sig: TimeSignature, pub measure: i64 }
```

`linear` records a ramp to the next event. `measure` is the **zero-based bar
index** at that position, so measure numbering survives meter changes.

### `TimeMap`

```rust
pub struct TimeMap { pub tempos: Vec<TempoEvent>, pub meters: Vec<MeterEvent> }
```

Ordered, sorted and deduplicated on construction, and **always non-empty** — the
default is a single 120 bpm 4/4 event at position zero.

- `meter_at(qn)`, `tempo_at(qn)`, `qn_to_seconds(qn)`.
- `bar_of(qn)` (zero-based), `bar_start(bar)`, `position_in_bar(qn)`,
  `metric_weight(qn)`.
- `bar_grid(start, end)`, `beat_grid(start, end, beats)`.
- `hash_hex()` — SHA-256 over the canonical JSON, used for change detection
  inside the server.

### The rest of the time vocabulary

| Brief concept | Where it lives |
|---|---|
| Tempo events | `TempoEvent`, `TimeMap::tempos` |
| Time-signature events | `MeterEvent`, `TimeMap::meters` |
| Measure positions | `MeterEvent::measure`, `TimeMap::bar_of`, `bar_start` |
| QN positions | `BeatTime` everywhere |
| Item-relative positions | `item_relative_start_qn` / `item_relative_end_qn` on the IPC snapshot note; the engine works in project QN |
| Pickup positions | `Phrase::is_pickup`, `PhraseAnalysis::pickup`, `loop_engine::pickup_length`, `LoopReport::pickup_qn` |
| Loop start and end | `LoopSpan { start, end, intent }`, `LoopReport::loop_start` / `loop_end` |
| Tail metadata | `loop_engine::tail_length`, `LoopReport::tail_qn` |

---

## 2. Pitch

**Spelling is preserved independently of sounding pitch.** G♯ and A♭ are the
same key on a piano and two different musical objects here, because they imply
different harmonic roles and different resolutions. Nothing in the model ever
collapses them.

### `Letter`

`C D E F G A B`. `natural_pc()` gives `C=0 D=2 E=4 F=5 G=7 A=9 B=11`;
`diatonic_index()` gives `C=0 … B=6`; `step(n)` walks the letter cycle and
returns the octave carry, which is what makes spelling-correct transposition
possible.

### `Accidental`

```rust
pub struct Accidental(pub i8);   // chromatic alteration in semitones
```

Constants `DOUBLE_FLAT`, `FLAT`, `NATURAL`, `SHARP`, `DOUBLE_SHARP`. Wider
alterations are *representable* — the type does not forbid them — but
`is_common()` is false outside `-2..=2`.

- `ascii()` → `"bb"`, `"b"`, `""`, `"#"`, `"##"`, and `"(+3)"` for anything
  wider.
- `unicode()` → `"𝄫"`, `"♭"`, `""`, `"♯"`, `"𝄪"`.

### `SpelledPitch`

```rust
pub struct SpelledPitch { pub letter: Letter, pub accidental: Accidental, pub octave: i32 }
```

Scientific pitch notation: **C4 = MIDI 60**.

- `midi()` — sounding pitch. `pitch_class()` — `0..11`.
- `diatonic_step()` — `octave·7 + letter.diatonic_index()`. This is the
  coordinate that makes interval arithmetic spelling-aware.
- `parse("C#4")` / `parse("Db4")` / `parse("C♯4")` / `parse("Bbb3")` — octave
  required. `parse_class("C#")` for a bare pitch class.
- `to_ascii()`, `to_unicode()`, `class_ascii()`.
- `transpose(interval)` — **spelling-correct**: transposing C♯ up a major third
  gives E♯, not F.
- `transpose_diatonic(steps, scale)` — moves within a scale.
- `from_midi(midi, ctx)` — the reverse direction, which is genuinely ambiguous;
  a `SpellingContext` supplies the key context that resolves it.
- `enharmonic_respell(target_letter)` — an explicit respelling request.
- `is_valid_midi()` — `0..=127`.
- Ordered by `(midi, diatonic_step)`, so C♯4 and D♭4 sort adjacently but never
  compare equal.

### `SpellingContext`

```rust
pub struct SpellingContext {
    pub tonic_pc: Option<i32>,
    pub prefer_flats: bool,
    pub scale_pcs: Vec<i32>,
    pub scale_spelling: Vec<(Letter, Accidental)>,
}
```

The preferred-accidental context used when converting MIDI to a spelling. A
`ScaleInstance` can produce one directly, which is how key context reaches the
speller.

### Microtonality

**Not supported, and not claimed.** Version one is 12-tone equal temperament.
A `cents: f64` field exists on `Note` for future compatibility and is **always
`0.0`** in this release. `SpelledPitch` deliberately carries no cents offset —
a spelling is a spelling.

---

## 3. Intervals

```rust
pub struct Interval { pub number: i32, pub quality: IntervalQuality, pub octaves: i32 }
pub enum IntervalQuality { Diminished(u8), Minor, Perfect, Major, Augmented(u8) }
```

`number` is the simple diatonic number `1..=7` (unison through seventh);
`octaves` adds compounding. `Diminished` and `Augmented` carry a degree, so
doubly-diminished intervals are representable.

`Interval` is **ascending-only** by design. Direction is a separate concern:

```rust
pub struct SignedInterval { pub interval: Interval, pub descending: bool }
```

which is what melodic motion uses.

- Constants `P1 m2 M2 m3 M3 P4 A4 d5 P5 m6 M6 m7 M7 P8`.
- `new(number, quality)` returns `None` for impossible combinations (there is no
  "perfect third").
- `semitones()` — chromatic distance. `diatonic_steps()` — `number - 1 + 7·octaves`.
- `between(a, b) -> SignedInterval` — the spelled interval between two pitches.
- `from_semitones_default(n)` — conventional spelling when no context exists.
- `name()` → `"M3"`, `"P5"`, `"A4"`, `"d7"`, `"M9"`. `parse()` is the inverse.
- `is_perfect_consonance()` (P1 P5 P8), `is_imperfect_consonance()`
  (m3 M3 m6 M6), `is_dissonant()`.
- `inverted()`, `simple()`.

Consonance classification is a property of the *spelled* interval, which is why
an augmented fourth and a diminished fifth are distinguishable here even though
both are six semitones.

---

## 4. Scales

```rust
pub struct ScaleDef {
    pub id: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub semitones: Vec<i32>,               // ascending from 0, excludes the octave
    pub degree_spelling: Vec<String>,      // "1", "b3", "#4", "b7" — parallel to semitones
    pub characteristic_degrees: Vec<String>,
    pub parent: Option<String>,
    pub mode_of: Option<(String, usize)>,
    pub common_chords: Vec<String>,
    pub tension_degrees: Vec<String>,
    pub family: String,                    // "diatonic", "pentatonic", "symmetric", ...
    pub source_refs: Vec<String>,
}
```

Loaded from `knowledge/scales.json` (35 definitions). `characteristic_degrees`
is what makes modal identification possible — the degrees that distinguish a
mode from its relatives — and `tension_degrees` is what makes avoid-note
reasoning possible.

```rust
pub struct ScaleInstance { pub def: ScaleDef, pub tonic: (Letter, Accidental) }
```

A definition rooted on a concrete tonic **with concrete spelling**:
`pitch_classes()`, `spelled_degrees()`, `contains_pc(pc)`, `degree_of_pc(pc)`,
`spell(midi)`, `spelling_context()`, `nearest_degree(midi)`.

---

## 5. Notes and voices

```rust
pub type NoteId = u32;

pub struct Note {
    pub id: NoteId,                    // stable within the analysis
    pub pitch: SpelledPitch,           // spelled
    pub midi: i32,                     // authoritative sounding pitch, 0..=127
    pub cents: f64,                    // future microtonal field; always 0.0 in v1
    pub onset: BeatTime,
    pub duration: BeatTime,            // strictly > 0
    pub velocity: u8,                  // 1..=127
    pub channel: u8,                   // 0..=15
    pub muted: bool,
    pub selected: bool,
    pub voice: VoiceId,
    pub role: NoteRole,
    pub articulation: Option<String>,
    pub salience: f64,                 // 0..=1, structural weight
    pub nct: Vec<NctHypothesis>,       // note several, not one
    pub confidence: f64,
    pub source_ppq: Option<f64>,       // REAPER provenance
    pub source_take: Option<String>,   // REAPER provenance
}
```

`pitch` and `midi` are both present on purpose: `midi` is what sounds, `pitch`
is what it means, and the two are kept consistent but distinct.

`validate()` enforces the invariants — MIDI range, positive duration, velocity
and channel ranges — and `end()`, `overlaps(other)` and `contains(qn)` are the
temporal predicates the engines use.

**Voice identity.**

```rust
pub struct VoiceId(pub u16);
pub const VOICE_UNASSIGNED: VoiceId = VoiceId(u16::MAX);
```

Voice identity is carried on the note and maintained **across chords**. A chord
progression is a set of connected voices moving through time, not a sequence of
unordered pitch sets; that is what makes voice-leading analysis meaningful and
what `harmony-engine`'s voicing stage preserves.

**Roles.**

```rust
pub enum NoteRole { Melody, Counter, Bass, Harmony, Pedal, Ornament, Percussion, Unknown }
```

**Non-chord-tone classification.**

```rust
pub enum NctKind {
    ChordTone, Passing, Neighbor, Suspension, Retardation, Anticipation,
    Appoggiatura, EscapeTone, PedalTone, ChromaticApproach, Enclosure, Decorative,
}
pub struct NctHypothesis { pub kind: NctKind, pub confidence: f64, pub rationale: String }
```

A note carries a **vector** of hypotheses, each with a confidence and a
rationale, because a genuinely ambiguous tone deserves an ambiguous answer.
Classification uses surrounding motion, metric position and duration — never
pitch membership alone.

**`NoteSet`** is the shared analysis unit:

```rust
pub struct NoteSet { pub notes: Vec<Note>, pub time_map: TimeMap, pub origin: NoteOrigin }
pub struct NoteOrigin { pub take_guid: Option<String>, pub item_guid: Option<String>, pub track_guid: Option<String> }
```

`sorted(notes, time_map)` orders by `(onset, midi, id)`. Queries:
`span()`, `is_monophonic()`, `polyphony_at(qn)`, `max_polyphony()`,
`sounding_at(qn)`, `by_voice(v)`, `highest_line()`, `lowest_line()`,
`pitch_class_durations()` (the duration-weighted 12-bin profile key inference
uses), and `hash_hex()`.

`NoteOrigin` is the provenance link back into REAPER, and it is how a generated
part is always traceable to the take it was derived from.

---

## 6. Chords

**A chord is never just pitch classes.** This is the type that most repays
reading, because almost every chord bug in a music system comes from a model
that threw away meaning too early.

### `ChordDegree`

```rust
pub struct ChordDegree { pub number: u8, pub alter: i8 }   // 1..=13 plus semitone alteration
```

- `parse("b9")`, `parse("#11")`, `parse("13")`, `parse("b5")`.
- `semitones_from_root()` — compound (a 9th is 14 semitones).
- `simple_semitones()` — modulo 12.
- `interval()` — the spelled `Interval`.

A `#11` and a `b5` are **different degrees**, not two names for six semitones,
and the model keeps them apart all the way through.

### `ChordSpec` — the identity of a chord

```rust
pub struct ChordSpec {
    pub root: (Letter, Accidental),
    pub triad: TriadQuality,
    pub seventh: SeventhQuality,
    pub extensions: Vec<ChordDegree>,    // 9/11/13, stacking implied
    pub added: Vec<ChordDegree>,         // add9, add4, 6 — no seventh implied
    pub alterations: Vec<ChordDegree>,   // b5 #5 b9 #9 #11 b13
    pub omissions: Vec<u8>,              // 1, 3, 5
    pub bass: Option<(Letter, Accidental)>,
    pub alt_dominant: bool,              // the symbol literally said "alt"
}
```

```rust
pub enum TriadQuality { Major, Minor, Diminished, Augmented, Sus2, Sus4, Power, Omitted }
pub enum SeventhQuality { None, Major, Minor, Diminished, AugmentedMajor }
```

A dominant seventh is `TriadQuality::Major` + `SeventhQuality::Minor`. There is
no separate "dominant" triad quality, because dominant-ness is a combination,
not a primitive.

The separation of `extensions` from `added` is what makes `Cadd9` and `C9`
different objects: `add9` adds a ninth without implying a seventh; `9` implies
the seventh below it. Likewise `C6` is an added sixth and not a rootless
thirteenth. `omissions` records what is deliberately absent, so a rootless
voicing still knows it is a rootless *that* chord.

Behaviour:

- `chord_tones()` → `Vec<(ChordDegree, (Letter, Accidental))>` — the **semantic**
  chord tones, root-position, ordered by degree, each still knowing which degree
  it is.
- `pitch_classes()` — the lossy view, available when you genuinely want it.
- `guide_tones()` — the 3rd and 7th, or their substitutes.
- Family predicates: `is_dominant_family()`, `is_major_family()`,
  `is_minor_family()`, `is_diminished_family()`, `is_suspended()`.
- `has_degree(n)`, `degree_of_pc(pc)`, `root_pc()`, `bass_pc()`.
- `transpose(interval)` — spelling-correct.
- `render_ascii()` / `render_unicode()` — the canonical symbol.
  **`render_ascii` round-trips through `parse`**, which is enforced by test.
- `family_id()` → `"major" | "minor" | "dominant" | "half_dim" | "dim" | "sus" | "power" | "aug"`.

### Symbol parsing

```rust
pub mod symbol {
    pub fn parse(s: &str) -> Result<ChordSpec, SymbolError>;
    pub struct SymbolError { pub input: String, pub position: usize, pub message: String }
}
```

Backed by 65 symbol aliases in `knowledge/chord_symbols.json`. ASCII and Unicode
accidentals normalize consistently, and the error reports the position where
parsing failed rather than a bare "invalid chord".

### `ChordEvent` — a chord placed in time

```rust
pub struct ChordEvent {
    pub id: u32,
    pub spec: ChordSpec,
    pub onset: BeatTime,
    pub duration: BeatTime,
    pub inversion: u8,
    pub function: Option<HarmonicFunction>,
    pub roman: Option<String>,
    pub local_tonic: Option<((Letter, Accidental), String)>,   // (tonic, scale id)
    pub voicing: Option<Voicing>,
    pub confidence: f64,
    pub inference_source: String,     // "parsed_symbol" | "detected" | "generated" | "user"
    pub original_symbol: Option<String>,
}
```

This is the type that satisfies the brief's chord requirements in full. Note in
particular:

- `function` and `roman` are `Option` — an unanalysed chord says so rather than
  guessing.
- `local_tonic` carries **both** a tonic and a scale id, so a modal context is
  expressible without pretending it is a major or minor key.
- `inference_source` records *where the chord came from*, so a user-supplied
  symbol is never confused with a detected or generated one.
- `original_symbol` preserves what was written, alongside the canonical
  rendering from `spec`.
- `confidence` is present on every chord, always.

```rust
pub enum HarmonicFunction {
    Tonic, Predominant, Dominant, Applied, Chromatic, Modal, Pedal,
    Passing, Neighbor, Unclassified,
}
```

`Modal` and `Unclassified` exist deliberately: not every chord in every idiom
has a functional role, and the model refuses to invent one.

### Voicing and register

```rust
pub struct Voicing { pub pitches: Vec<SpelledPitch>, pub family: VoicingFamily, pub voices: Vec<VoiceId> }

pub enum VoicingFamily {
    Close, Open, Drop2, Drop3, Shell, Rootless, Spread,
    Quartal, Quintal, Cluster, Power, UpperStructure, Pedal,
}
```

13 families. A `Voicing` carries **spelled** pitches — so register and spelling
are both concrete — and a parallel `voices` vector, which is what preserves
voice identity from chord to chord. Register is expressed by the octaves of the
spelled pitches; there is no separate register field, because the pitches
already say it exactly.

---

## 7. Musical structures

Everything above the note. These are plain data types with JSON forms; the
analysis and generation crates fill them in.

### Phrase and motive

```rust
pub struct Phrase {
    pub id: u32, pub start: BeatTime, pub end: BeatTime,
    pub notes: Vec<NoteId>, pub cadence: Option<CadenceKind>,
    pub is_pickup: bool, pub confidence: f64,
}
```

**Subphrases use the same type.** `PhraseAnalysis` (in `music-analysis`) carries
`phrases`, `subphrases`, `motives`, `gaps` and an optional `pickup` — the
distinction is structural position, not a different shape.

```rust
pub struct Motive {
    pub id: u32,
    pub occurrences: Vec<MotiveOccurrence>,
    pub interval_profile: Vec<i32>,
    pub rhythm_profile: Vec<BeatTime>,
    pub salience: f64,
}
pub struct MotiveOccurrence {
    pub start: BeatTime, pub notes: Vec<NoteId>,
    pub transposition: i32, pub transform: MotiveTransform,
}
pub enum MotiveTransform {
    Exact, Transposed, Sequence, Inverted, Retrograde, Augmented, Diminished, Varied,
}
```

A motive is defined by its **interval and rhythm profiles**, not by its pitches,
which is why a transposed or inverted restatement is recognised as the same
motive.

### Cadence

```rust
pub enum CadenceKind {
    PerfectAuthentic, ImperfectAuthentic, Half, Plagal,
    Deceptive, Phrygian, Modal, None,
}
```

`Modal` is a first-class cadence kind, not a failure to classify — a modal
arrival is a real cadence and is scored as one.

### Key and harmonic regions, and the modal centre

```rust
pub struct KeyRegion {
    pub start: BeatTime, pub end: BeatTime,
    pub tonic: (Letter, Accidental),
    pub scale_id: String,
    pub confidence: f64,
    pub evidence: Vec<String>,
    pub is_tonicization: bool,
}
pub struct HarmonicRegion {
    pub start: BeatTime, pub end: BeatTime,
    pub label: String, pub function: HarmonicFunction,
}
```

**The modal centre is a `KeyRegion` whose `scale_id` names a mode**, not a
separate type. That is deliberate: a Dorian centre on D is exactly "tonic D,
collection Dorian" and gains nothing from a parallel type hierarchy. The
`is_tonicization` flag separates a passing tonicization from a real modulation,
and `evidence` records why the region was proposed.

The ranked, whole-selection view lives in `music_analysis::KeyAnalysis` with its
`KeyCandidate` list, each carrying a per-source `evidence` breakdown and an
`is_modal` flag.

### Pedal point

A pedal is represented where it acts, rather than as a standalone object:

- `NoteRole::Pedal` on the sustaining note.
- `HarmonicFunction::Pedal` on chords sounding over it.
- `VoicingFamily::Pedal` for a voicing built around one.
- `BassMotion::Pedal` (in `harmony-engine`) to request one.
- `NctKind::PedalTone` when a tone is classified against the prevailing harmony.

The loop engine additionally reasons about a pedal continuing across a wrap,
which is one of the ways a `ModalDrone` loop is judged compatible without any
dominant resolution.

### Harmony event

The harmony event **is** `ChordEvent` (§6): a chord with an onset, a duration,
an inversion, a function, a local tonic, a voicing, a confidence and a recorded
inference source.

### Voice-leading connection

```rust
pub struct VoiceLeadingConnection {
    pub from: SpelledPitch, pub to: SpelledPitch,
    pub voice: VoiceId, pub motion: MotionKind, pub semitones: i32,
}
pub enum MotionKind { Static, Step, Leap }
pub enum RelativeMotion { Parallel, Similar, Contrary, Oblique, Static }
```

`MotionKind` classifies one voice's own move; `RelativeMotion` classifies a pair
of voices against each other. Both are needed: a leap is a property of a line,
a parallel fifth is a property of a relationship.

### Section, arrangement role and arrangement layer

```rust
pub struct Section {
    pub id: String, pub start: BeatTime, pub end: BeatTime,
    pub role: String, pub energy: f64,
}

pub enum ArrangementRole {
    Lead, Counterlead, Bass, HarmonicBed, Pad, Comping, Pulse, Ostinato,
    Riff, Percussion, Impact, Transition, Texture, Ambience, Ornament, EarCandy,
}
```

16 roles. `ArrangementRole::all()` returns them in that order, and
`id()`/`parse()` map to and from the snake-case ids the knowledge bundle uses.

**An arrangement layer is a `Part`** (§8) — a role plus the notes that realise
it plus its instrument profile, channel and polyphony. The planning-time view is
`arrangement_engine::RoleAssignment`, which records the chosen pattern id, the
chosen instrument profile, the allocated register window, the measured density
and polyphony, the priority tier (0 foreground, 1 midground, 2 background), the
sections it sounds in, and a rationale.

Four roles — `Pulse`, `Percussion`, `Ornament` and `EarCandy` — have **no
pattern of their own** in the 28-entry catalogue. Rather than fabricate a
rhythm, the engine substitutes a catalogued pattern from an adjacent role along
a fixed preference chain (`ROLE_SUBSTITUTES`) and discloses the substitution in
the assignment's rationale.

### Energy curve

An energy curve is `Vec<(BeatTime, f64)>` — a piecewise-linear control over the
span, empty meaning flat. It appears as `ArrangementParams::energy_curve` on the
way in and `ArrangementPlan::energy` on the way out, and per-section as
`Section::energy`. It is a control signal rather than an object, which is why it
has no dedicated struct.

### Loop policy

Loop intent and boundary policy are two separate decisions:

```rust
pub enum LoopIntent {
    ClosedTonic, OpenDominant, ModalDrone, SeamlessColor, TransitionReady, OneShotEnding,
}
```

*What the loop is meant to do at its wrap.* This is what the loop audit scores
against, and it is why V-to-I across the wrap is required for `ClosedTonic` and
explicitly **not** required for `ModalDrone`.

```rust
pub enum CarryPolicy { Split, Carry, Truncate, Rearticulate }   // loop_engine::boundary
```

*What happens to a note that crosses the boundary.* `LoopSpan { start, end,
intent }` binds an intent to a span.

The audit result is `LoopReport` (§8).

---

## 8. Candidates, plans and traces

### `ScoreVector`

A named, insertion-ordered set of score components plus a separately stored
weighted total, so a caller always sees both what was measured and how it was
combined. The 13 components are fixed:

```
melody_fit                       harmonic_coherence
functional_or_modal_coherence    voice_leading
extension_appropriateness        style_match
phrase_direction                 bass_quality
arrangement_clarity              loop_compatibility
complexity_target                chromaticism_target
candidate_diversity
```

Every resolved style profile must supply a weight for exactly these 13, which
validation enforces.

### `RuleApplication` and `DecisionTrace`

```rust
pub struct RuleApplication {
    pub rule_id: String,
    pub status: RuleStatus,               // Applied | Bypassed | NotApplicable | Violated
    pub score_delta: f64,
    pub matched_conditions: Vec<String>,
    pub matched_exceptions: Vec<String>,
    pub source_ids: Vec<String>,
    pub explanation: String,
}

pub struct DecisionTrace {
    pub candidate_id: String, pub analysis_id: String, pub snapshot_id: String,
    pub knowledge_version: String, pub profile_id: String, pub seed: u64,
    pub assumptions: Vec<String>, pub confidence: f64, pub score: ScoreVector,
    pub rule_applications: Vec<RuleApplication>, pub warnings: Vec<Warning>,
    pub source_ids: Vec<String>, pub explanation: String,
    pub rejected_alternatives: Vec<String>,
}

pub struct Warning { pub code: String, pub message: String, pub severity: Severity }
pub enum Severity { Info, Minor, Moderate, Major }
```

`RuleStatus::Bypassed` and `NotApplicable` are distinct on purpose: a rule that
an exception disarmed is a different fact from a rule whose inputs were never
present. `rejected_alternatives` records what the solver considered and did not
choose, which is often the most informative part of a trace.

A trace pins the exact knowledge version, profile, seed, analysis and snapshot,
so any candidate can be regenerated byte-for-byte.

### `Candidate` and `Part`

```rust
pub struct Candidate {
    pub id: String,
    pub kind: CandidateKind,          // Harmonization | Reharmonization | Voicing | Arrangement
    pub label: String,                // short human name, e.g. "Functional & cadence-directed"
    pub strategy: String,             // stable id, e.g. "functional"
    pub chords: Vec<ChordEvent>,
    pub parts: Vec<Part>,
    pub trace: DecisionTrace,
    pub loop_report: Option<LoopReport>,
    pub created_at: String,
    pub expires_at: String,
}

pub struct Part {
    pub role: ArrangementRole, pub name: String, pub notes: Vec<Note>,
    pub instrument_profile: Option<String>, pub channel: u8, pub polyphonic: bool,
}
```

`label` is for humans and `strategy` is for machines; diversity selection works
on `strategy`, and a client can group candidates by it. `expires_at` is the TTL
the session store enforces.

### `LoopReport`

```rust
pub struct LoopReport {
    pub intent: Option<LoopIntent>,
    pub loop_start: BeatTime, pub loop_end: BeatTime,
    pub compatible: bool, pub score: f64,
    pub harmonic_wrap: String, pub bass_wrap: String, pub voice_leading_wrap: String,
    pub hanging_notes: Vec<NoteId>, pub crossing_notes: Vec<NoteId>,
    pub pickup_qn: BeatTime, pub tail_qn: BeatTime,
    pub findings: Vec<Warning>, pub repairs: Vec<String>, pub confidence: f64,
}
```

`hanging_notes` are notes left sounding at the wrap; `crossing_notes` are notes
that span the boundary. The three `*_wrap` strings are human descriptions of
what happens harmonically, in the bass and in the voices at the seam.
`repairs` names suggested fixes — it never applies them.

### `EditPlan`

```rust
pub struct EditPlan {
    pub plan_id: String, pub candidate_id: String, pub transaction_id: String,
    pub base_snapshot_id: String, pub base_snapshot_hash: String, pub project_uuid: String,
    pub knowledge_version: String, pub undo_label: String,
    pub operations: Vec<EditOperation>,
    pub preconditions: Vec<Precondition>,
    pub expected_outputs: Vec<ExpectedOutput>,
}

pub enum EditOperation {
    CreateFolderTrack { temp_id, name, tags },
    CreateTrack       { temp_id, parent, name, tags },
    CreateMidiItem    { temp_id, track, start_qn, end_qn, tags, muted },
    InsertNotes       { item, notes },
    SetTrackMute      { track, muted },
    CreateRegion      { name, start_qn, end_qn },
    CreateMidiSend    { from_track, to_track_guid },
}

pub enum Precondition {
    ProjectUuid(String), StateChangeCount(i64),
    ItemGuidExists(String), TakeGuidExists(String),
    MidiHash(String), TempoMapHash(String),
    ItemBounds { start_qn: f64, end_qn: f64 },
}

pub struct PlannedNote {
    pub start_qn: BeatTime, pub end_qn: BeatTime, pub pitch: i32,
    pub velocity: u8, pub channel: u8, pub muted: bool, pub spelling: String,
}
pub struct ExpectedOutput { pub temp_id: String, pub kind: String, pub note_count: Option<usize> }
```

The plan is the *only* way anything is written to a REAPER project. Seven
operations, seven precondition kinds, no escape hatch. `PlannedNote::spelling`
records the intended spelling for the plan and the trace — MIDI itself stores
only the number, so the spelling would otherwise be lost at the boundary.

`undo_label` must begin with the literal prefix `QLabs MCP: `; the bridge
refuses any other label and the undo tool refuses to undo any entry lacking it.

**Note the two wire forms.** `EditPlan::to_json` is the MCP-facing
representation, where `BeatTime` is a rational string. The bridge's wire form
differs in three places (the precondition discriminator, numeric `BeatTime`, and
tags as pairs) and is produced only by `reaper_ipc::plan::plan_to_wire`. See
[`ARCHITECTURE.md` §7](ARCHITECTURE.md#7-reaper-ipc).

---

## 9. Identifiers and errors

`music_domain::ids` builds on `qjson::uuid` and fixes the namespaces used for
content-derived, stable identifiers:

```
qlabs.reapermcp.analysis     qlabs.reapermcp.candidate
qlabs.reapermcp.plan         qlabs.reapermcp.transaction
qlabs.reapermcp.snapshot     qlabs.reapermcp.knowledge
qlabs.reapermcp.fixture
```

`uuid_from_name(namespace, name)` is a stable v5-style derivation: the same
input yields the same id forever. That is why an identical analysis request
returns an identical `analysis://{id}` and why golden tests are possible.
`IdFactory` supplies runtime ids where a fresh one is wanted.

```rust
pub struct DomainError { pub code: String, pub message: String }
```

Codes come from the brief's error vocabulary, shared with the IPC layer so that
one error string means one thing everywhere in the product.

---

## 10. Where each brief concept lives

A direct index against the brief's required model, so nothing can quietly go
missing.

| Required | Type or field |
|---|---|
| Tempo events | `TempoEvent`, `TimeMap::tempos` |
| Time-signature events | `MeterEvent`, `TimeMap::meters` |
| Measure positions | `MeterEvent::measure`, `TimeMap::bar_of` / `bar_start` |
| QN positions | `BeatTime` |
| Item-relative positions | IPC snapshot note fields; engine works in project QN |
| Pickup positions | `Phrase::is_pickup`, `PhraseAnalysis::pickup`, `LoopReport::pickup_qn` |
| Loop start / end | `LoopSpan`, `LoopReport::loop_start` / `loop_end` |
| Tail metadata | `loop_engine::tail_length`, `LoopReport::tail_qn` |
| Spelled pitch | `SpelledPitch { letter, accidental, octave }` |
| Accidental enum / integer | `Accidental(i8)` with named constants |
| MIDI pitch | `Note::midi`, `SpelledPitch::midi()` |
| Optional cents offset | `Note::cents` — **always 0.0 in v1** |
| Enharmonic distinction | `SpelledPitch` equality includes letter and accidental |
| Double-flat … double-sharp | `Accidental::DOUBLE_FLAT` … `DOUBLE_SHARP` |
| ASCII and Unicode parsing | `SpelledPitch::parse`, `Accidental::ascii`/`unicode` |
| Interval spelling | `Interval { number, quality, octaves }` |
| Diatonic interval number | `Interval::number`, `diatonic_steps()` |
| Chromatic semitone distance | `Interval::semitones()` |
| Pitch class | `SpelledPitch::pitch_class()` |
| Register | `SpelledPitch::octave`; voicing register from its pitches |
| Valid-spelling transposition | `SpelledPitch::transpose`, `ChordSpec::transpose` |
| Stable event id | `Note::id` (`NoteId`) |
| Onset / duration | `Note::onset`, `Note::duration` |
| Velocity / channel | `Note::velocity`, `Note::channel` |
| Mute / selection state | `Note::muted`, `Note::selected` |
| Source take | `Note::source_take`, `NoteSet::origin` |
| Voice identity | `Note::voice` (`VoiceId`), `Voicing::voices` |
| Musical role | `Note::role` (`NoteRole`) |
| Articulation metadata | `Note::articulation` |
| Structural salience | `Note::salience`, `SalienceReport` |
| NCT classification | `Note::nct` (`Vec<NctHypothesis>`) |
| Confidence | `Note::confidence`, and on chords, regions, candidates, traces |
| Chord root | `ChordSpec::root` |
| Chord bass | `ChordSpec::bass` |
| Triad quality | `ChordSpec::triad` |
| Seventh quality | `ChordSpec::seventh` |
| Added tones | `ChordSpec::added` |
| Extensions | `ChordSpec::extensions` |
| Alterations | `ChordSpec::alterations` |
| Suspensions | `TriadQuality::Sus2` / `Sus4` |
| Omissions | `ChordSpec::omissions` |
| Inversion | `ChordEvent::inversion` |
| Functional label | `ChordEvent::function` (`HarmonicFunction`) |
| Local tonic | `ChordEvent::local_tonic` |
| Scale / modal context | the scale id inside `local_tonic`; `ScaleInstance` |
| Voicing | `ChordEvent::voicing` (`Voicing`) |
| Register | octaves of `Voicing::pitches` |
| Duration | `ChordEvent::duration` |
| Confidence | `ChordEvent::confidence` |
| Source of inference | `ChordEvent::inference_source` |
| Original symbol | `ChordEvent::original_symbol` |
| Canonical symbol | `ChordSpec::render_ascii()` / `render_unicode()` |
| Motive | `Motive`, `MotiveOccurrence`, `MotiveTransform` |
| Phrase | `Phrase` |
| Subphrase | `Phrase` in `PhraseAnalysis::subphrases` |
| Cadence | `CadenceKind`, `Phrase::cadence` |
| Harmonic region | `HarmonicRegion` |
| Key region | `KeyRegion` |
| Modal centre | `KeyRegion` with a modal `scale_id`; `KeyCandidate::is_modal` |
| Pedal point | `NoteRole::Pedal`, `HarmonicFunction::Pedal`, `VoicingFamily::Pedal`, `NctKind::PedalTone`, `BassMotion::Pedal` |
| Harmony event | `ChordEvent` |
| Voice-leading connection | `VoiceLeadingConnection`, `MotionKind`, `RelativeMotion` |
| Section | `Section` |
| Arrangement role | `ArrangementRole` (16) |
| Arrangement layer | `Part`; planning view `RoleAssignment` |
| Energy curve | `Vec<(BeatTime, f64)>` in `ArrangementParams` / `ArrangementPlan`; `Section::energy` |
| Loop policy | `LoopIntent` (what the wrap should do) + `CarryPolicy` (what crossing notes do) |
| Candidate | `Candidate`, `CandidateKind` |
| Edit plan | `EditPlan`, `EditOperation`, `Precondition`, `ExpectedOutput`, `PlannedNote` |
| Decision trace | `DecisionTrace`, `RuleApplication`, `ScoreVector`, `Warning` |
