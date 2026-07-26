//! What actually happens at the loop seam.
//!
//! The seam is one point heard twice: the material just before the loop end is
//! followed immediately by the material at the loop start. Everything the brief
//! asks the audit to weigh — final harmony against initial harmony, bass
//! continuity, voice leading across the wrap, unresolved tendencies, pedal
//! continuity, harmonic rhythm at the wrap, percussion phase and layer removal
//! — is a measurement taken at that one point, and this module takes them.
//!
//! Nothing here decides whether the result is *good*. That depends on the loop
//! intent and lives in [`crate::intent`].

use crate::boundary::{is_marked_carry, LoopSpan};
use harmony_engine::keyctx::degree_for_semitones;
use music_domain::prelude::*;
use theory_kb::KnowledgeBase;

/// The tonal frame the wrap is judged inside.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyFrame {
    /// Spelled tonic.
    pub tonic: (Letter, Accidental),
    /// Knowledge-base scale id.
    pub scale_id: String,
    /// True when the passage is centred on a mode rather than a major or minor
    /// key. This is what makes `modal_center_is_active` true, and it is why a
    /// modal loop is never asked to produce a dominant.
    pub is_modal: bool,
    /// Confidence in the reading, `0.0..=1.0`.
    pub confidence: f64,
    /// Where the reading came from: `"analysis"`, `"chords"`, `"notes"` or
    /// `"default"`.
    pub source: &'static str,
}

/// The scale ids the chord-based reader considers, in preference order.
///
/// Two readings of the same collection — C lydian and G major name the same
/// seven pitch classes — are separated by the rest of the evidence, not by this
/// list; the order only breaks a genuine tie.
const CANDIDATE_SCALES: &[(&str, f64)] = &[
    ("major", 0.30),
    ("aeolian", 0.25),
    ("harmonic_minor", 0.15),
    ("mixolydian", 0.12),
    ("dorian", 0.10),
    ("lydian", 0.08),
    ("phrygian", 0.05),
];

/// The scale ids that make a passage genuinely modal rather than major/minor.
const MODAL_SCALE_IDS: &[&str] = &[
    "dorian",
    "phrygian",
    "lydian",
    "mixolydian",
    "aeolian",
    "locrian",
    "lydian_dominant",
    "mixolydian_flat6",
    "phrygian_dominant",
    "dorian_flat2",
    "dorian_sharp4",
    "lydian_augmented",
    "lydian_sharp2",
    "locrian_natural_2",
    "locrian_natural_6",
    "ionian_sharp5",
];

/// The scale ids the knowledge base treats as minor-key contexts when matching
/// `functions.json` entries.
const MINOR_LIKE: &[&str] = &[
    "aeolian",
    "natural_minor",
    "harmonic_minor",
    "melodic_minor",
    "dorian",
    "phrygian",
    "locrian",
    "minor_pentatonic",
];

impl KeyFrame {
    /// Pitch class of the tonic.
    pub fn tonic_pc(&self) -> i32 {
        (self.tonic.0.natural_pc() + i32::from(self.tonic.1 .0)).rem_euclid(12)
    }

    /// `"major"` or `"minor"`, the `functions.json` key context.
    pub fn key_context_id(&self) -> &'static str {
        if MINOR_LIKE.contains(&self.scale_id.as_str()) {
            "minor"
        } else {
            "major"
        }
    }

    /// The pitch classes of the active collection, when the knowledge base
    /// knows the scale.
    pub fn pitch_classes(&self, kb: &KnowledgeBase) -> Vec<i32> {
        match kb.scale(&self.scale_id) {
            Some(def) => ScaleInstance::new(def.clone(), self.tonic).pitch_classes(),
            None => Vec::new(),
        }
    }

    /// A spelling context for rendering MIDI pitches inside this key.
    pub fn spelling(&self, kb: &KnowledgeBase) -> SpellingContext {
        match kb.scale(&self.scale_id) {
            Some(def) => ScaleInstance::new(def.clone(), self.tonic).spelling_context(),
            None => SpellingContext::default(),
        }
    }

    /// A short human label, e.g. `"C major"`.
    pub fn label(&self) -> String {
        format!(
            "{}{} {}",
            self.tonic.0.as_char(),
            self.tonic.1.ascii(),
            self.scale_id.replace('_', " ")
        )
    }
}

/// The neutral fallback when there is nothing at all to read.
fn default_frame() -> KeyFrame {
    KeyFrame {
        tonic: (Letter::C, Accidental::NATURAL),
        scale_id: "major".to_string(),
        is_modal: false,
        confidence: 0.1,
        source: "default",
    }
}

/// Reads the tonal frame from whichever evidence the caller supplied.
///
/// An analysis is authoritative when one is present, because it weighed far
/// more evidence than a loop audit can see. Otherwise the chords are read, and
/// failing that the notes. The reading is never forced onto a major or minor
/// key: a modal collection wins when the evidence supports it, which is exactly
/// what keeps `modal_drone` from being judged against a dominant it never had.
pub fn infer_key(
    kb: &KnowledgeBase,
    notes: &NoteSet,
    chords: &[ChordEvent],
    time_map: &TimeMap,
    analysis: Option<&music_analysis::report::Analysis>,
) -> KeyFrame {
    if let Some(an) = analysis {
        if let Some(top) = an.key.candidates.first() {
            return KeyFrame {
                tonic: top.tonic,
                scale_id: top.scale_id.clone(),
                is_modal: top.is_modal || MODAL_SCALE_IDS.contains(&top.scale_id.as_str()),
                confidence: top.confidence,
                source: "analysis",
            };
        }
    }
    if !chords.is_empty() {
        return infer_key_from_chords(kb, chords);
    }
    if !notes.notes.is_empty() {
        let ka = music_analysis::key::analyze_key(kb, notes, time_map, None, None);
        if let Some(top) = ka.candidates.first() {
            return KeyFrame {
                tonic: top.tonic,
                scale_id: top.scale_id.clone(),
                is_modal: top.is_modal || MODAL_SCALE_IDS.contains(&top.scale_id.as_str()),
                confidence: top.confidence,
                source: "notes",
            };
        }
    }
    default_frame()
}

/// Spells a pitch class as the simplest letter/accidental pair.
fn spell_pc(pc: i32) -> (Letter, Accidental) {
    let pc = pc.rem_euclid(12);
    const NAMES: [(Letter, i8); 12] = [
        (Letter::C, 0),
        (Letter::C, 1),
        (Letter::D, 0),
        (Letter::E, -1),
        (Letter::E, 0),
        (Letter::F, 0),
        (Letter::F, 1),
        (Letter::G, 0),
        (Letter::A, -1),
        (Letter::A, 0),
        (Letter::B, -1),
        (Letter::B, 0),
    ];
    let (letter, alter) = NAMES[pc as usize];
    (letter, Accidental(alter))
}

/// Reads the tonal frame from a chord progression alone.
///
/// Weighted evidence, not a lookup: how much of the progression fits the
/// collection, whether the loop begins or ends on the candidate tonic, whether
/// a real dominant of that tonic is present, and how much time the tonic chord
/// itself occupies.
pub fn infer_key_from_chords(kb: &KnowledgeBase, chords: &[ChordEvent]) -> KeyFrame {
    let total: f64 = chords.iter().map(|c| c.duration.as_f64()).sum();
    if chords.is_empty() || total <= 0.0 {
        return default_frame();
    }
    let first_root = chords[0].spec.root_pc();
    let last_root = chords[chords.len() - 1].spec.root_pc();

    let mut best: Option<(f64, KeyFrame)> = None;
    for pc in 0..12 {
        for (scale_id, prior) in CANDIDATE_SCALES {
            let Some(def) = kb.scale(scale_id) else {
                continue;
            };
            let tonic = spell_pc(pc);
            let scale_pcs = ScaleInstance::new(def.clone(), tonic).pitch_classes();
            if scale_pcs.is_empty() {
                continue;
            }
            let mut fit = 0.0;
            let mut emphasis = 0.0;
            for c in chords {
                let pcs = c.spec.pitch_classes();
                if pcs.is_empty() {
                    continue;
                }
                let inside = pcs.iter().filter(|p| scale_pcs.contains(p)).count();
                fit += c.duration.as_f64() * (inside as f64 / pcs.len() as f64);
                if c.spec.root_pc() == pc {
                    emphasis += c.duration.as_f64();
                }
            }
            let fit = fit / total;
            let emphasis = emphasis / total;
            let dominant_present = chords.iter().any(|c| {
                c.spec.is_dominant_family() && c.spec.root_pc() == (pc + 7).rem_euclid(12)
            });
            let score = 4.0 * fit
                + f64::from(first_root == pc)
                + f64::from(last_root == pc)
                + if dominant_present { 1.2 } else { 0.0 }
                + emphasis
                + prior;
            let better = match &best {
                Some((s, _)) => score > *s,
                None => true,
            };
            if better {
                best = Some((
                    score,
                    KeyFrame {
                        tonic,
                        scale_id: (*scale_id).to_string(),
                        is_modal: MODAL_SCALE_IDS.contains(scale_id),
                        // 8.5 is the score a wholly unambiguous progression
                        // reaches; the ratio is reported rather than a flat 1.0.
                        confidence: (score / 8.5).clamp(0.1, 0.95),
                        source: "chords",
                    },
                ));
            }
        }
    }
    best.map(|(_, f)| f).unwrap_or_else(default_frame)
}

/// The harmonic function of a chord inside a key, read from
/// `knowledge/functions.json`.
///
/// The entry is matched on key context, scale degree and triad quality, with a
/// seventh-quality match preferred and a diatonic entry preferred over a
/// chromatic one. When no entry matches, a modal frame reports
/// [`HarmonicFunction::Modal`] rather than pretending the chord has a
/// common-practice function.
pub fn classify_function(kb: &KnowledgeBase, key: &KeyFrame, spec: &ChordSpec) -> HarmonicFunction {
    let degree = degree_for_semitones(spec.root_pc() - key.tonic_pc());
    let degree_text = degree.text();
    let context = key.key_context_id();

    let mut best: Option<(i32, &theory_kb::FunctionEntry)> = None;
    for entry in kb.functions() {
        if entry.key_context != context && entry.key_context != "any" {
            continue;
        }
        if entry.scale_degree.as_deref() != Some(degree_text.as_str()) {
            continue;
        }
        let triad_ok = kb
            .chord_quality(&entry.triad_quality)
            .map(|q| q.triad == spec.triad.id())
            .unwrap_or(false);
        if !triad_ok {
            continue;
        }
        let seventh_ok = kb
            .chord_quality(&entry.seventh_quality)
            .map(|q| q.seventh == spec.seventh.id())
            .unwrap_or(false);
        let mut rank = 0;
        if seventh_ok == spec.seventh.is_present() {
            rank += 2;
        }
        if entry.category == "diatonic" {
            rank += 1;
        }
        if entry.key_context == context {
            rank += 1;
        }
        if best.as_ref().map(|(r, _)| rank > *r).unwrap_or(true) {
            best = Some((rank, entry));
        }
    }
    if let Some((_, entry)) = best {
        return harmony_engine::keyctx::function_class(&entry.function_class);
    }

    // Safety net for collections `functions.json` does not enumerate.
    let root_pc = spec.root_pc();
    if root_pc == key.tonic_pc() {
        return HarmonicFunction::Tonic;
    }
    if spec.is_dominant_family() && root_pc == (key.tonic_pc() + 7).rem_euclid(12) {
        return HarmonicFunction::Dominant;
    }
    if key.is_modal {
        HarmonicFunction::Modal
    } else {
        HarmonicFunction::Unclassified
    }
}

/// The `functions.json` class id for a domain function.
pub fn function_id(f: HarmonicFunction) -> &'static str {
    harmony_engine::factbuild::function_class_id(f)
}

/// A percussion pattern's relationship to the loop grid.
#[derive(Clone, Debug, PartialEq)]
pub struct PercussionPhase {
    /// How many percussion onsets fall inside the loop.
    pub hits: usize,
    /// The shortest span the pattern repeats over.
    pub period_qn: BeatTime,
    /// How far the first hit sits after the loop start.
    pub phase_offset_qn: BeatTime,
    /// True when the pattern's period divides the loop length exactly and the
    /// first hit lands on the loop point.
    pub aligned: bool,
}

/// A layer that carries a chord tone nothing else is playing, and that stops at
/// the wrap.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerRemoval {
    /// Index into the caller's `parts` slice.
    pub part_index: usize,
    /// The part's display name.
    pub part_name: String,
    /// The degree that disappears, e.g. `"3"` or `"b7"`.
    pub degree: String,
}

/// Everything measured at the seam.
#[derive(Clone, Debug, Default)]
pub struct WrapObservation {
    /// Function of the loop's last chord.
    pub final_function: Option<HarmonicFunction>,
    /// Function of the loop's first chord.
    pub first_function: Option<HarmonicFunction>,
    /// Symbol of the last chord, for the report text.
    pub final_symbol: Option<String>,
    /// Symbol of the first chord, for the report text.
    pub first_symbol: Option<String>,
    /// Sounding MIDI pitches at the loop end, ascending.
    pub end_pitches: Vec<i32>,
    /// Sounding MIDI pitches at the loop start, ascending.
    pub start_pitches: Vec<i32>,
    /// Lowest sounding pitch at the loop end.
    pub bass_end: Option<i32>,
    /// Lowest sounding pitch at the loop start.
    pub bass_start: Option<i32>,
    /// Signed bass motion across the seam in semitones.
    pub bass_interval: Option<i32>,
    /// True when the bass motion is larger than a perfect fifth.
    pub bass_leaps: bool,
    /// Pitch classes common to both sides of the seam.
    pub common_tone_pcs: Vec<i32>,
    /// True when the two sonorities share any pitch class.
    pub common_tone_available: bool,
    /// True when a shared pitch class is actually held in the same voice.
    pub common_tone_retained: bool,
    /// True when every voice could cross the seam by step or by holding.
    pub stepwise_available: bool,
    /// Total semitone travel of the cheapest non-crossing connection.
    pub total_motion: i32,
    /// Largest single-voice move of that connection.
    pub max_leap: i32,
    /// Tendency tones left unresolved across the seam, described.
    pub unresolved_tendencies: Vec<String>,
    /// True when a pedal or drone is sounding.
    pub pedal_active: bool,
    /// True when that pedal is still sounding on both sides of the seam.
    pub pedal_continues: bool,
    /// Length of the last harmonic slot.
    pub slot_before: Option<BeatTime>,
    /// Length of the first harmonic slot.
    pub slot_after: Option<BeatTime>,
    /// True when those two lengths differ.
    pub harmonic_rhythm_changes: bool,
    /// Percussion phase metadata, when the material has percussion.
    pub percussion: Option<PercussionPhase>,
    /// A layer whose removal at the wrap costs an essential chord tone.
    pub layer_removal: Option<LayerRemoval>,
    /// True when the material carries parts, so the layer question is
    /// answerable at all.
    pub layers_known: bool,
    /// How stable the collection is across the loop, `0.0..=1.0`.
    pub collection_stability: f64,
}

/// True when the note can be heard.
fn sounds(note: &Note) -> bool {
    !note.muted && note.duration.is_positive()
}

/// The simultaneity heard immediately before the loop end.
fn final_sonority(notes: &NoteSet, span: &LoopSpan) -> Vec<i32> {
    let last_onset = notes
        .notes
        .iter()
        .filter(|n| sounds(n) && n.onset < span.end && n.end() > span.start)
        .map(|n| n.onset)
        .max();
    let Some(t) = last_onset else {
        return Vec::new();
    };
    let mut out: Vec<i32> = notes
        .notes
        .iter()
        .filter(|n| sounds(n) && n.onset <= t && n.end() > t)
        .map(|n| n.midi)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The simultaneity heard at the loop start.
fn initial_sonority(notes: &NoteSet, span: &LoopSpan) -> Vec<i32> {
    let first_onset = notes
        .notes
        .iter()
        .filter(|n| sounds(n) && n.onset >= span.start && n.onset < span.end)
        .map(|n| n.onset)
        .min();
    let t = match first_onset {
        Some(t) => t,
        None => span.start,
    };
    let mut out: Vec<i32> = notes
        .notes
        .iter()
        .filter(|n| sounds(n) && n.onset <= t && n.end() > t)
        .map(|n| n.midi)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// A chord's pitch classes stacked from `bass_pc` upwards near middle C.
///
/// Used when the caller supplied chords but no notes: the wrap still has to be
/// judged as a chord connection, so the chord is given a plain close position
/// rather than being skipped.
pub fn nominal_pitches(spec: &ChordSpec) -> Vec<i32> {
    let bass = spec.bass_pc().rem_euclid(12);
    let mut pcs = spec.pitch_classes();
    pcs.sort_unstable();
    pcs.dedup();
    let mut out = vec![48 + bass];
    let mut previous = 48 + bass;
    for pc in pcs {
        if pc.rem_euclid(12) == bass {
            continue;
        }
        let mut midi = 48 + pc.rem_euclid(12);
        while midi <= previous {
            midi += 12;
        }
        out.push(midi);
        previous = midi;
    }
    out.sort_unstable();
    out
}

/// The cheapest non-crossing connection between two sonorities.
///
/// Returns `(pairs, total_motion, max_leap)`. Voices are allowed to converge
/// but never to cross, which is what makes the answer a *voice leading* rather
/// than an arbitrary pairing.
pub fn connect(from: &[i32], to: &[i32]) -> (Vec<(i32, i32)>, i32, i32) {
    if from.is_empty() || to.is_empty() {
        return (Vec::new(), 0, 0);
    }
    let n = from.len();
    let m = to.len();
    let inf = i32::MAX / 4;
    // dp[i][j] = cheapest way to map from[i..] onto to[j..], monotonically.
    let mut dp = vec![vec![inf; m + 1]; n + 1];
    let mut choice = vec![vec![0usize; m + 1]; n + 1];
    for j in 0..=m {
        dp[n][j] = 0;
    }
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            let mut best = inf;
            let mut best_k = j;
            for k in j..m {
                let cost = (from[i] - to[k]).abs();
                let rest = dp[i + 1][k];
                if rest >= inf {
                    continue;
                }
                if cost + rest < best {
                    best = cost + rest;
                    best_k = k;
                }
            }
            dp[i][j] = best;
            choice[i][j] = best_k;
        }
    }
    let mut pairs = Vec::with_capacity(n);
    let mut j = 0usize;
    for (i, f) in from.iter().enumerate() {
        let k = choice[i][j];
        pairs.push((*f, to[k]));
        j = k;
    }
    let total = pairs.iter().map(|(a, b)| (a - b).abs()).sum();
    let max = pairs.iter().map(|(a, b)| (a - b).abs()).max().unwrap_or(0);
    (pairs, total, max)
}

/// True when some non-crossing connection moves every voice by at most a whole
/// step.
pub fn stepwise_connection_available(from: &[i32], to: &[i32]) -> bool {
    if from.is_empty() || to.is_empty() {
        return false;
    }
    let n = from.len();
    let m = to.len();
    let inf = i32::MAX / 4;
    let mut dp = vec![vec![inf; m + 1]; n + 1];
    for j in 0..=m {
        dp[n][j] = 0;
    }
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            let mut best = inf;
            for k in j..m {
                let rest = dp[i + 1][k];
                if rest >= inf {
                    continue;
                }
                let here = (from[i] - to[k]).abs().max(rest);
                if here < best {
                    best = here;
                }
            }
            dp[i][j] = best;
        }
    }
    dp[0][0] <= 2
}

/// Tendency tones on the far side of the seam that never arrive.
///
/// Two tendencies are checked because they are the two the knowledge base
/// names: the leading tone must rise a semitone to the tonic, and a chordal
/// seventh must fall by step.
fn unresolved_tendencies(
    key: &KeyFrame,
    final_spec: Option<&ChordSpec>,
    pairs: &[(i32, i32)],
) -> Vec<String> {
    let mut out = Vec::new();
    let tonic = key.tonic_pc();
    let leading = (tonic + 11).rem_euclid(12);
    for (from, to) in pairs {
        if from.rem_euclid(12) == leading && (to - from) != 1 && to.rem_euclid(12) != tonic {
            out.push(format!(
                "the leading tone {} does not rise to the tonic across the wrap",
                SpelledPitch::from_midi(*from, None).to_ascii()
            ));
        }
    }
    if let Some(spec) = final_spec {
        if spec.seventh.is_present() {
            let seventh_pc = spec
                .chord_tones()
                .iter()
                .find(|(d, _)| d.number == 7)
                .map(|(d, _)| (spec.root_pc() + d.simple_semitones()).rem_euclid(12));
            if let Some(pc) = seventh_pc {
                let resolves = pairs
                    .iter()
                    .filter(|(f, _)| f.rem_euclid(12) == pc)
                    .any(|(f, t)| (t - f) == -1 || (t - f) == -2);
                let present = pairs.iter().any(|(f, _)| f.rem_euclid(12) == pc);
                if present && !resolves {
                    out.push(format!(
                        "the chordal seventh of {} does not fall by step across the wrap",
                        spec.render_ascii()
                    ));
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// True when the note behaves as a pedal or drone.
fn is_pedal(note: &Note, span: &LoopSpan) -> bool {
    if note.role == NoteRole::Pedal {
        return true;
    }
    let half = span.length().scale(1, 2);
    half.is_positive() && note.duration >= half
}

/// Percussion phase metadata, when the material has percussion at all.
fn percussion_phase(notes: &NoteSet, span: &LoopSpan) -> Option<PercussionPhase> {
    let length = span.length();
    if !length.is_positive() {
        return None;
    }
    let mut onsets: Vec<BeatTime> = notes
        .notes
        .iter()
        .filter(|n| sounds(n) && n.role == NoteRole::Percussion && span.contains(n.onset))
        .map(|n| n.onset - span.start)
        .collect();
    if onsets.is_empty() {
        return None;
    }
    onsets.sort();
    onsets.dedup();

    let mut period = length;
    for divisor in [16i64, 12, 8, 6, 4, 3, 2] {
        let candidate = length.scale(1, divisor);
        if !candidate.is_positive() {
            continue;
        }
        if !length.divides_evenly_by(candidate) {
            continue;
        }
        let invariant = onsets
            .iter()
            .all(|o| onsets.contains(&(*o + candidate).rem_euclid(length)));
        if invariant {
            period = candidate;
            break;
        }
    }
    let phase_offset = onsets[0];
    Some(PercussionPhase {
        hits: onsets.len(),
        period_qn: period,
        phase_offset_qn: phase_offset,
        aligned: phase_offset.is_zero() && length.divides_evenly_by(period),
    })
}

/// A part that is the only source of a guide tone at the wrap and then stops.
fn layer_removal(
    parts: &[Part],
    chords: &[ChordEvent],
    span: &LoopSpan,
) -> (bool, Option<LayerRemoval>) {
    if parts.is_empty() || chords.is_empty() {
        return (false, None);
    }
    let last = &chords[chords.len() - 1];
    let first = &chords[0];
    let guide: Vec<(String, i32)> = last
        .spec
        .chord_tones()
        .iter()
        .filter(|(d, _)| d.number == 3 || d.number == 7 || d.number == 4)
        .map(|(d, _)| {
            (
                d.to_string(),
                (last.spec.root_pc() + d.simple_semitones()).rem_euclid(12),
            )
        })
        .collect();
    if guide.is_empty() {
        return (true, None);
    }

    let sounding_in = |part: &Part, from: BeatTime, to: BeatTime| -> Vec<i32> {
        let mut pcs: Vec<i32> = part
            .notes
            .iter()
            .filter(|n| sounds(n) && n.onset < to && n.end() > from)
            .map(|n| n.midi.rem_euclid(12))
            .collect();
        pcs.sort_unstable();
        pcs.dedup();
        pcs
    };

    let final_from = last.onset.max(span.start);
    let final_to = span.end;
    let first_from = span.start;
    let first_to = first.end().min(span.end);

    for (degree, pc) in &guide {
        let owners: Vec<usize> = (0..parts.len())
            .filter(|i| sounding_in(&parts[*i], final_from, final_to).contains(pc))
            .collect();
        if owners.len() != 1 {
            continue;
        }
        let i = owners[0];
        let present_at_start = !sounding_in(&parts[i], first_from, first_to).is_empty();
        if !present_at_start {
            return (
                true,
                Some(LayerRemoval {
                    part_index: i,
                    part_name: parts[i].name.clone(),
                    degree: degree.clone(),
                }),
            );
        }
    }
    (true, None)
}

/// How much of the progression stays inside one collection.
fn collection_stability(kb: &KnowledgeBase, key: &KeyFrame, chords: &[ChordEvent]) -> f64 {
    let pcs = key.pitch_classes(kb);
    if chords.is_empty() || pcs.is_empty() {
        return 0.0;
    }
    let total: f64 = chords.iter().map(|c| c.duration.as_f64()).sum();
    if total <= 0.0 {
        return 0.0;
    }
    let mut inside = 0.0;
    for c in chords {
        let tones = c.spec.pitch_classes();
        if tones.is_empty() {
            continue;
        }
        let hit = tones.iter().filter(|p| pcs.contains(p)).count();
        inside += c.duration.as_f64() * (hit as f64 / tones.len() as f64);
    }
    (inside / total).clamp(0.0, 1.0)
}

/// Takes every measurement at the seam.
#[allow(clippy::too_many_arguments)] // Each argument is a distinct input the caller owns.
pub fn observe(
    kb: &KnowledgeBase,
    key: &KeyFrame,
    span: &LoopSpan,
    notes: &NoteSet,
    chords: &[ChordEvent],
    parts: &[Part],
) -> WrapObservation {
    let mut o = WrapObservation::default();

    let inside: Vec<&ChordEvent> = chords
        .iter()
        .filter(|c| c.onset < span.end && c.end() > span.start)
        .collect();
    let final_chord = inside.last().copied();
    let first_chord = inside.first().copied();

    if let Some(c) = final_chord {
        o.final_function = Some(
            c.function
                .unwrap_or_else(|| classify_function(kb, key, &c.spec)),
        );
        o.final_symbol = Some(
            c.original_symbol
                .clone()
                .unwrap_or_else(|| c.spec.render_ascii()),
        );
        o.slot_before = Some(c.duration);
    }
    if let Some(c) = first_chord {
        o.first_function = Some(
            c.function
                .unwrap_or_else(|| classify_function(kb, key, &c.spec)),
        );
        o.first_symbol = Some(
            c.original_symbol
                .clone()
                .unwrap_or_else(|| c.spec.render_ascii()),
        );
        o.slot_after = Some(c.duration);
    }
    o.harmonic_rhythm_changes = match (o.slot_before, o.slot_after) {
        (Some(a), Some(b)) => a != b,
        _ => false,
    };

    // Real notes are preferred; chords give a nominal close position when the
    // caller supplied harmony without a realisation.
    o.end_pitches = final_sonority(notes, span);
    if o.end_pitches.is_empty() {
        if let Some(c) = final_chord {
            o.end_pitches = nominal_pitches(&c.spec);
        }
    }
    o.start_pitches = initial_sonority(notes, span);
    if o.start_pitches.is_empty() {
        if let Some(c) = first_chord {
            o.start_pitches = nominal_pitches(&c.spec);
        }
    }

    o.bass_end = o.end_pitches.first().copied();
    o.bass_start = o.start_pitches.first().copied();
    o.bass_interval = match (o.bass_end, o.bass_start) {
        (Some(a), Some(b)) => Some(b - a),
        _ => None,
    };
    o.bass_leaps = o.bass_interval.map(|i| i.abs() > 7).unwrap_or(false);

    let (pairs, total, max) = connect(&o.end_pitches, &o.start_pitches);
    o.total_motion = total;
    o.max_leap = max;
    o.stepwise_available = stepwise_connection_available(&o.end_pitches, &o.start_pitches);
    o.common_tone_retained = pairs.iter().any(|(a, b)| (a - b).rem_euclid(12) == 0);

    let end_pcs: Vec<i32> = o.end_pitches.iter().map(|m| m.rem_euclid(12)).collect();
    let start_pcs: Vec<i32> = o.start_pitches.iter().map(|m| m.rem_euclid(12)).collect();
    let mut common: Vec<i32> = end_pcs
        .iter()
        .copied()
        .filter(|p| start_pcs.contains(p))
        .collect();
    common.sort_unstable();
    common.dedup();
    o.common_tone_available = !common.is_empty();
    o.common_tone_pcs = common;

    o.unresolved_tendencies =
        unresolved_tendencies(key, final_chord.map(|c| &c.spec), &pairs);

    let pedals: Vec<&Note> = notes
        .notes
        .iter()
        .filter(|n| sounds(n) && is_pedal(n, span))
        .collect();
    o.pedal_active = !pedals.is_empty();
    o.pedal_continues = pedals.iter().any(|n| {
        (n.onset < span.end && n.end() > span.end)
            || (n.end() == span.end
                && notes.notes.iter().any(|m| {
                    sounds(m)
                        && m.onset == span.start
                        && m.midi.rem_euclid(12) == n.midi.rem_euclid(12)
                }))
    }) || pedals.iter().any(|n| is_marked_carry(n));

    o.percussion = percussion_phase(notes, span);
    let (layers_known, removal) = layer_removal(parts, chords, span);
    o.layers_known = layers_known;
    o.layer_removal = removal;
    o.collection_stability = collection_stability(kb, key, chords);
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use music_domain::symbol;

    fn kb() -> &'static KnowledgeBase {
        KnowledgeBase::embedded()
    }

    fn chord(sym: &str, onset: i64, dur: i64, id: u32) -> ChordEvent {
        let mut e = ChordEvent::new(
            id,
            symbol::parse(sym).expect("symbol"),
            BeatTime::from_quarters(onset),
            BeatTime::from_quarters(dur),
        );
        e.original_symbol = Some(sym.to_string());
        e
    }

    #[test]
    fn a_functional_progression_reads_as_a_major_key() {
        let chords = vec![
            chord("C", 0, 4, 0),
            chord("Am", 4, 4, 1),
            chord("F", 8, 4, 2),
            chord("G7", 12, 4, 3),
        ];
        let key = infer_key_from_chords(kb(), &chords);
        assert_eq!(key.tonic.0, Letter::C);
        assert_eq!(key.scale_id, "major");
        assert!(!key.is_modal);
    }

    #[test]
    fn a_planing_progression_reads_as_modal() {
        let chords = vec![
            chord("D", 0, 4, 0),
            chord("C", 4, 4, 1),
            chord("G", 8, 4, 2),
            chord("C", 12, 4, 3),
        ];
        let key = infer_key_from_chords(kb(), &chords);
        assert!(
            key.is_modal,
            "planing major triads have no functional dominant; the reading was {}",
            key.label()
        );
    }

    #[test]
    fn functions_come_from_the_knowledge_base() {
        let key = KeyFrame {
            tonic: (Letter::C, Accidental::NATURAL),
            scale_id: "major".to_string(),
            is_modal: false,
            confidence: 0.9,
            source: "chords",
        };
        let g7 = symbol::parse("G7").unwrap();
        let c = symbol::parse("C").unwrap();
        let f = symbol::parse("F").unwrap();
        assert_eq!(
            classify_function(kb(), &key, &g7),
            HarmonicFunction::Dominant
        );
        assert_eq!(classify_function(kb(), &key, &c), HarmonicFunction::Tonic);
        assert_eq!(
            classify_function(kb(), &key, &f),
            HarmonicFunction::Predominant
        );
    }

    #[test]
    fn connect_never_crosses_voices() {
        let (pairs, total, max) = connect(&[55, 59, 62, 65], &[48, 52, 55, 60]);
        assert_eq!(pairs.len(), 4);
        let targets: Vec<i32> = pairs.iter().map(|(_, t)| *t).collect();
        assert!(targets.windows(2).all(|w| w[0] <= w[1]));
        assert!(total >= max);
    }

    #[test]
    fn a_planing_wrap_is_stepwise() {
        assert!(stepwise_connection_available(&[60, 64, 67], &[62, 66, 69]));
        assert!(!stepwise_connection_available(&[60, 64, 67], &[72, 76, 79]));
    }

    #[test]
    fn nominal_pitches_stack_upwards_from_the_bass() {
        let p = nominal_pitches(&symbol::parse("G7").unwrap());
        assert_eq!(p[0], 48 + 7);
        assert!(p.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(p.len(), 4);
    }

    #[test]
    fn an_empty_connection_is_not_an_error() {
        let (pairs, total, max) = connect(&[], &[60]);
        assert!(pairs.is_empty());
        assert_eq!((total, max), (0, 0));
        assert!(!stepwise_connection_available(&[], &[60]));
    }

    #[test]
    fn key_frame_reports_its_context_and_label() {
        let frame = KeyFrame {
            tonic: (Letter::D, Accidental::NATURAL),
            scale_id: "dorian".to_string(),
            is_modal: true,
            confidence: 0.8,
            source: "chords",
        };
        assert_eq!(frame.key_context_id(), "minor");
        assert_eq!(frame.label(), "D dorian");
        assert_eq!(frame.tonic_pc(), 2);
    }
}
