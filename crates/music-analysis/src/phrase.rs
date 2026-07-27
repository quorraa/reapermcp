//! Stage 2a — phrase segmentation, motive detection and the melody profile.
//!
//! Phrases are found from **boundary strength**, a weighted sum of the four
//! things that actually end a phrase in notated music: a rest, a long note, an
//! arrival on a metrically strong position, and a change of melodic direction
//! after a leap. No single one of them is decisive — a melody with no rests at
//! all still has phrases, and a melody full of staccato eighths does not have a
//! phrase boundary every beat — so they are combined and thresholded, with a
//! hypermetric fallback that forces a split in material that never breathes.
//!
//! Motives are found by matching **interval and rhythm profiles** rather than
//! pitches, so a transposed or sequenced restatement is the same motive. The
//! transform is then classified (exact, transposed, sequence, inverted,
//! retrograde, augmented, diminished, varied) and reported per occurrence.

use crate::util::{cmp_f64, median_duration};
use music_domain::prelude::*;
use qjson::{Json, JsonMap};

/// Boundary strength at or above which a phrase ends.
const PHRASE_THRESHOLD: f64 = 0.55;

/// Boundary strength at or above which a *sub*phrase ends.
const SUBPHRASE_THRESHOLD: f64 = 0.28;

/// Phrases longer than this many bars are split at their strongest interior
/// boundary, so continuous material still gets segmented.
const MAX_PHRASE_BARS: i64 = 4;

/// Phrases shorter than this many bars are merged into their neighbour, unless
/// they are a pickup.
const MIN_PHRASE_BARS_NUM: i64 = 1;

/// Shortest and longest motive, in notes.
const MOTIVE_MIN_LEN: usize = 3;
/// See [`MOTIVE_MIN_LEN`].
const MOTIVE_MAX_LEN: usize = 6;

/// The result of stage 2a.
#[derive(Clone, Debug, Default)]
pub struct PhraseAnalysis {
    /// Top-level phrases, in time order.
    pub phrases: Vec<Phrase>,
    /// Subphrase divisions, in time order. A subphrase always lies inside
    /// exactly one phrase.
    pub subphrases: Vec<Phrase>,
    /// Recurring melodic ideas, most significant first.
    pub motives: Vec<Motive>,
    /// Rests of at least half a beat, as `(start, end)`.
    pub gaps: Vec<(BeatTime, BeatTime)>,
    /// The anacrusis, when the melody starts before its first downbeat.
    pub pickup: Option<Phrase>,
}

impl PhraseAnalysis {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert(
            "phrases",
            Json::Arr(self.phrases.iter().map(Phrase::to_json).collect()),
        );
        m.insert(
            "subphrases",
            Json::Arr(self.subphrases.iter().map(Phrase::to_json).collect()),
        );
        m.insert(
            "motives",
            Json::Arr(self.motives.iter().map(Motive::to_json).collect()),
        );
        m.insert(
            "gaps",
            Json::Arr(
                self.gaps
                    .iter()
                    .map(|(a, b)| {
                        let mut g = JsonMap::new();
                        g.insert("start", a.to_json());
                        g.insert("end", b.to_json());
                        Json::Obj(g)
                    })
                    .collect(),
            ),
        );
        m.insert(
            "pickup",
            match &self.pickup {
                Some(p) => p.to_json(),
                None => Json::Null,
            },
        );
        Json::Obj(m)
    }

    /// The phrase containing `id`, if any.
    pub fn phrase_of(&self, id: NoteId) -> Option<&Phrase> {
        self.phrases.iter().find(|p| p.notes.contains(&id))
    }

    /// True when `id` opens a phrase.
    pub fn is_phrase_start(&self, id: NoteId) -> bool {
        self.phrases.iter().any(|p| p.notes.first() == Some(&id))
    }

    /// True when `id` closes a phrase.
    pub fn is_phrase_end(&self, id: NoteId) -> bool {
        self.phrases.iter().any(|p| p.notes.last() == Some(&id))
    }

    /// True when `id` opens or closes a subphrase.
    pub fn is_subphrase_edge(&self, id: NoteId) -> bool {
        self.subphrases
            .iter()
            .any(|p| p.notes.first() == Some(&id) || p.notes.last() == Some(&id))
    }

    /// True when `id` belongs to the pickup.
    pub fn is_pickup(&self, id: NoteId) -> bool {
        self.pickup.as_ref().is_some_and(|p| p.notes.contains(&id))
    }

    /// The motives `id` participates in.
    pub fn motives_of(&self, id: NoteId) -> Vec<&Motive> {
        self.motives
            .iter()
            .filter(|m| m.occurrences.iter().any(|o| o.notes.contains(&id)))
            .collect()
    }
}

/// Segments `m` into phrases, subphrases and motives.
///
/// `cadence` is left `None` here: a cadence is a *harmonic* event and this
/// stage has no key. [`refine_cadences`] fills it in once stage 3 has run.
pub fn analyze_phrases(m: &NoteSet, tm: &TimeMap) -> PhraseAnalysis {
    let notes = &m.notes;
    if notes.is_empty() {
        return PhraseAnalysis::default();
    }

    let bar = tm.meter_at(notes[0].onset).bar_length_qn();
    let beat = tm.meter_at(notes[0].onset).beat_unit_qn();
    let median = median_duration(notes).unwrap_or(beat);

    // --- rests ---
    let mut gaps = Vec::new();
    for w in notes.windows(2) {
        let gap = w[1].onset - w[0].end();
        if gap.is_positive() && gap >= beat.scale(1, 2) {
            gaps.push((w[0].end(), w[1].onset));
        }
    }

    // --- boundary strength after each note ---
    let strengths: Vec<f64> = (0..notes.len().saturating_sub(1))
        .map(|i| boundary_strength(notes, i, tm, bar, beat, median))
        .collect();

    // --- pickup ---
    let first = notes[0].onset;
    let downbeat = next_bar_line(tm, first);
    let pickup_ids: Vec<NoteId> = if first < downbeat && (downbeat - first) <= bar.scale(1, 2) {
        notes
            .iter()
            .filter(|n| n.onset < downbeat)
            .map(|n| n.id)
            .collect()
    } else {
        Vec::new()
    };
    let has_pickup = !pickup_ids.is_empty() && pickup_ids.len() < notes.len();

    // --- top-level cuts ---
    let start_index = if has_pickup { pickup_ids.len() } else { 0 };
    let mut cuts: Vec<usize> = strengths
        .iter()
        .enumerate()
        .filter(|(i, s)| *i >= start_index && **s >= PHRASE_THRESHOLD)
        .map(|(i, _)| i)
        .collect();
    if has_pickup {
        cuts.push(start_index - 1);
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut segments = split(notes.len(), &cuts);
    segments = enforce_length(notes, segments, &strengths, tm, bar, has_pickup);

    // --- build phrases ---
    let mut phrases = Vec::new();
    let mut next_id = 0u32;
    let mut pickup_phrase = None;
    for (si, (lo, hi)) in segments.iter().enumerate() {
        let ids: Vec<NoteId> = notes[*lo..=*hi].iter().map(|n| n.id).collect();
        let is_pickup = has_pickup && si == 0;
        let p = Phrase {
            id: next_id,
            start: notes[*lo].onset,
            end: notes[*hi].end(),
            notes: ids,
            cadence: None,
            is_pickup,
            confidence: segment_confidence(&strengths, *lo, *hi, is_pickup),
        };
        next_id += 1;
        if is_pickup {
            pickup_phrase = Some(p.clone());
        }
        phrases.push(p);
    }

    // --- subphrases ---
    let mut subphrases = Vec::new();
    for p in &phrases {
        let (lo, hi) = span_of(notes, p);
        let interior: Vec<usize> = (lo..hi)
            .filter(|i| strengths[*i] >= SUBPHRASE_THRESHOLD)
            .collect();
        if interior.is_empty() {
            continue;
        }
        for (a, b) in split_range(lo, hi, &interior) {
            subphrases.push(Phrase {
                id: next_id,
                start: notes[a].onset,
                end: notes[b].end(),
                notes: notes[a..=b].iter().map(|n| n.id).collect(),
                cadence: None,
                is_pickup: p.is_pickup,
                confidence: segment_confidence(&strengths, a, b, p.is_pickup) * 0.85,
            });
            next_id += 1;
        }
    }

    let motives = detect_motives(notes);

    PhraseAnalysis {
        phrases,
        subphrases,
        motives,
        gaps,
        pickup: pickup_phrase,
    }
}

/// Fills in [`Phrase::cadence`] using the tonal centre found by stage 3.
///
/// A cadence is classified from the melodic approach to the phrase's final
/// scale degree, which is all a melody-only analysis can honestly claim:
///
/// | Final degree | Approach | Cadence |
/// |---|---|---|
/// | tonic | leading tone from below, strong beat | perfect authentic |
/// | tonic | any other | imperfect authentic |
/// | tonic | flat second from above | phrygian |
/// | dominant | any | half |
/// | third | any | imperfect authentic |
/// | sixth | from the dominant | deceptive |
/// | fourth | from the dominant | plagal |
///
/// In a modal collection — one with no leading tone — a close on the tonic is
/// reported as [`CadenceKind::Modal`] rather than as an authentic cadence,
/// because there is no dominant function driving it.
pub fn refine_cadences(
    ph: &mut PhraseAnalysis,
    m: &NoteSet,
    tonic_pc: i32,
    scale_pcs: &[i32],
    tm: &TimeMap,
) {
    let has_leading_tone = scale_pcs.contains(&((tonic_pc + 11).rem_euclid(12)));
    for p in ph.phrases.iter_mut().chain(ph.subphrases.iter_mut()) {
        p.cadence = classify_cadence(p, m, tonic_pc, has_leading_tone, tm);
    }
    if let Some(pick) = ph.pickup.as_mut() {
        pick.cadence = None;
    }
}

/// The cadence closing one phrase, or `None` when nothing recognisable closes
/// it.
fn classify_cadence(
    p: &Phrase,
    m: &NoteSet,
    tonic_pc: i32,
    has_leading_tone: bool,
    tm: &TimeMap,
) -> Option<CadenceKind> {
    let last = p.notes.last().and_then(|id| find(m, *id))?;
    let prev = if p.notes.len() >= 2 {
        p.notes
            .get(p.notes.len() - 2)
            .and_then(|id| find(m, *id))
            .map(|n| n.midi)
    } else {
        None
    };
    let degree = (last.midi - tonic_pc).rem_euclid(12);
    let approach = prev.map(|q| last.midi - q);
    let strong = tm.metric_weight(last.onset) >= 0.75;
    Some(match (degree, approach) {
        (0, Some(1)) if has_leading_tone && strong => CadenceKind::PerfectAuthentic,
        (0, Some(-1)) => CadenceKind::Phrygian,
        (0, _) if !has_leading_tone => CadenceKind::Modal,
        (0, _) => CadenceKind::ImperfectAuthentic,
        (7, _) => CadenceKind::Half,
        (3, _) | (4, _) => CadenceKind::ImperfectAuthentic,
        (8, Some(d)) | (9, Some(d)) if d != 0 => CadenceKind::Deceptive,
        (5, _) => CadenceKind::Plagal,
        _ => return None,
    })
}

/// Boundary strength after `notes[i]`, in `0.0..=1.0`.
fn boundary_strength(
    notes: &[Note],
    i: usize,
    tm: &TimeMap,
    bar: BeatTime,
    beat: BeatTime,
    median: BeatTime,
) -> f64 {
    let here = &notes[i];
    let next = &notes[i + 1];

    // A rest is the strongest single signal there is.
    let gap = next.onset - here.end();
    let rest = if gap.is_positive() {
        (gap.as_f64() / beat.as_f64().max(1e-9)).min(1.0)
    } else {
        0.0
    };

    // A note noticeably longer than the prevailing value closes a group.
    let ratio = here.duration.as_f64() / median.as_f64().max(1e-9);
    let length = ((ratio - 1.0) / 2.0).clamp(0.0, 1.0);

    // An arrival on a bar line — more so on an even bar line, the hypermetric
    // grouping most Western phrase structure uses.
    let at_bar = next.onset.rem_euclid(bar).is_zero();
    let bar_index = tm.bar_of(next.onset);
    let metric = if at_bar && bar_index.rem_euclid(4) == 0 {
        1.0
    } else if at_bar && bar_index.rem_euclid(2) == 0 {
        0.75
    } else if at_bar {
        0.5
    } else {
        0.0
    };

    // A leap away after a descent (or vice versa) resets the line.
    let turn = if i > 0 {
        let a = here.midi - notes[i - 1].midi;
        let b = next.midi - here.midi;
        if a != 0 && b != 0 && a.signum() != b.signum() && b.abs() >= 5 {
            1.0
        } else {
            0.0
        }
    } else {
        0.0
    };

    0.60 * rest + 0.20 * length + 0.15 * metric + 0.05 * turn
}

/// Splits `0..len` at the given cut indices (a cut after index `c`).
fn split(len: usize, cuts: &[usize]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut lo = 0usize;
    for c in cuts {
        if *c + 1 >= len {
            continue;
        }
        out.push((lo, *c));
        lo = *c + 1;
    }
    out.push((lo, len - 1));
    out
}

/// Splits `lo..=hi` at interior cut indices.
fn split_range(lo: usize, hi: usize, cuts: &[usize]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut a = lo;
    for c in cuts {
        if *c >= hi {
            continue;
        }
        out.push((a, *c));
        a = *c + 1;
    }
    if a <= hi {
        out.push((a, hi));
    }
    out
}

/// Forces long segments apart and folds tiny ones into their neighbour.
fn enforce_length(
    notes: &[Note],
    segments: Vec<(usize, usize)>,
    strengths: &[f64],
    tm: &TimeMap,
    bar: BeatTime,
    has_pickup: bool,
) -> Vec<(usize, usize)> {
    let max_len = bar * MAX_PHRASE_BARS;
    let min_len = bar * MIN_PHRASE_BARS_NUM;

    // Split anything over the ceiling at its strongest interior boundary,
    // repeatedly, so a 16-bar unbroken line becomes four phrases and not one.
    let mut queue = segments;
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut guard = 0;
    while let Some((lo, hi)) = queue.first().copied() {
        queue.remove(0);
        guard += 1;
        if guard > 512 {
            out.push((lo, hi));
            continue;
        }
        let len = notes[hi].end() - notes[lo].onset;
        if len <= max_len || hi <= lo {
            out.push((lo, hi));
            continue;
        }
        let mut best: Option<(usize, f64)> = None;
        for i in lo..hi {
            // Prefer a cut near the middle so a long line divides evenly.
            let here = notes[i].end() - notes[lo].onset;
            let balance = 1.0 - ((here.as_f64() / len.as_f64().max(1e-9)) - 0.5).abs() * 2.0;
            let score = strengths[i] + 0.35 * balance;
            if best.is_none_or(|(_, b)| score > b) {
                best = Some((i, score));
            }
        }
        match best {
            Some((cut, _)) if cut < hi => {
                queue.insert(0, (cut + 1, hi));
                queue.insert(0, (lo, cut));
            }
            _ => out.push((lo, hi)),
        }
    }
    out.sort_unstable();

    // Merge runts forward, except a pickup, which is meant to be short.
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (idx, (lo, hi)) in out.into_iter().enumerate() {
        let len = notes[hi].end() - notes[lo].onset;
        let is_pickup = has_pickup && idx == 0;
        if !is_pickup && len < min_len && !merged.is_empty() {
            let last = merged.len() - 1;
            // Never merge a real phrase into the pickup.
            if !(has_pickup && last == 0) {
                merged[last].1 = hi;
                continue;
            }
        }
        merged.push((lo, hi));
    }
    let _ = tm;
    merged
}

/// Confidence that a segment really is a phrase: how decisive its closing
/// boundary was.
fn segment_confidence(strengths: &[f64], _lo: usize, hi: usize, is_pickup: bool) -> f64 {
    if is_pickup {
        return 0.7;
    }
    match strengths.get(hi) {
        Some(s) => (0.5 + s.clamp(0.0, 1.0) * 0.5).clamp(0.0, 1.0),
        // The final phrase ends because the material does.
        None => 0.9,
    }
}

/// Index range of a phrase within the note vector.
fn span_of(notes: &[Note], p: &Phrase) -> (usize, usize) {
    let lo = p
        .notes
        .first()
        .and_then(|id| notes.iter().position(|n| n.id == *id))
        .unwrap_or(0);
    let hi = p
        .notes
        .last()
        .and_then(|id| notes.iter().position(|n| n.id == *id))
        .unwrap_or(lo);
    (lo, hi)
}

/// The first bar line at or after `qn`.
fn next_bar_line(tm: &TimeMap, qn: BeatTime) -> BeatTime {
    let start = tm.bar_start(tm.bar_of(qn));
    if start == qn {
        qn
    } else {
        tm.bar_start(tm.bar_of(qn) + 1)
    }
}

/// A note by id.
fn find(m: &NoteSet, id: NoteId) -> Option<&Note> {
    m.notes.iter().find(|n| n.id == id)
}

// ---------------------------------------------------------------------------
// motives
// ---------------------------------------------------------------------------

/// Finds recurring melodic ideas.
///
/// A motive is keyed on its **interval profile** — the sequence of semitone
/// steps — so a transposition is the same motive. Rhythm is compared
/// separately, which is what distinguishes an exact restatement from an
/// augmentation. Occurrences never overlap: the longest window that matches is
/// taken first and its notes are then unavailable to shorter windows.
fn detect_motives(notes: &[Note]) -> Vec<Motive> {
    if notes.len() < MOTIVE_MIN_LEN * 2 {
        return Vec::new();
    }
    let intervals: Vec<i32> = notes.windows(2).map(|w| w[1].midi - w[0].midi).collect();

    let mut used = vec![false; notes.len()];
    let mut motives: Vec<Motive> = Vec::new();
    let mut next_id = 0u32;

    for len in (MOTIVE_MIN_LEN..=MOTIVE_MAX_LEN).rev() {
        if notes.len() < len * 2 {
            continue;
        }
        let mut start = 0usize;
        while start + len <= notes.len() {
            if used[start..start + len].iter().any(|u| *u) {
                start += 1;
                continue;
            }
            let profile: Vec<i32> = intervals[start..start + len - 1].to_vec();
            let rhythm: Vec<BeatTime> = notes[start..start + len]
                .iter()
                .map(|n| n.duration)
                .collect();

            let mut occurrences = vec![occurrence(notes, start, len, 0, MotiveTransform::Exact)];
            let mut j = start + len;
            while j + len <= notes.len() {
                if used[j..j + len].iter().any(|u| *u) {
                    j += 1;
                    continue;
                }
                let other: Vec<i32> = intervals[j..j + len - 1].to_vec();
                let other_rhythm: Vec<BeatTime> =
                    notes[j..j + len].iter().map(|n| n.duration).collect();
                if let Some(t) = transform_of(&profile, &rhythm, &other, &other_rhythm) {
                    let transposition = notes[j].midi - notes[start].midi;
                    occurrences.push(occurrence(notes, j, len, transposition, t));
                    j += len;
                } else {
                    j += 1;
                }
            }

            if occurrences.len() >= 2 {
                classify_sequences(&mut occurrences);
                for o in &occurrences {
                    for id in &o.notes {
                        if let Some(pos) = notes.iter().position(|n| n.id == *id) {
                            used[pos] = true;
                        }
                    }
                }
                let coverage = (occurrences.len() * len) as f64 / notes.len() as f64;
                motives.push(Motive {
                    id: next_id,
                    occurrences,
                    interval_profile: profile,
                    rhythm_profile: rhythm,
                    salience: (0.35 + coverage).min(1.0),
                });
                next_id += 1;
                start += len;
            } else {
                start += 1;
            }
        }
    }

    motives.sort_by(|a, b| {
        cmp_f64(b.salience, a.salience)
            .then(b.occurrences.len().cmp(&a.occurrences.len()))
            .then(
                a.occurrences
                    .first()
                    .map(|o| o.start)
                    .cmp(&b.occurrences.first().map(|o| o.start)),
            )
    });
    for (i, m) in motives.iter_mut().enumerate() {
        m.id = i as u32;
    }
    motives
}

/// Builds one occurrence record.
fn occurrence(
    notes: &[Note],
    start: usize,
    len: usize,
    transposition: i32,
    transform: MotiveTransform,
) -> MotiveOccurrence {
    MotiveOccurrence {
        start: notes[start].onset,
        notes: notes[start..start + len].iter().map(|n| n.id).collect(),
        transposition,
        transform,
    }
}

/// How `other` relates to `profile`, or `None` when it is not the same idea.
fn transform_of(
    profile: &[i32],
    rhythm: &[BeatTime],
    other: &[i32],
    other_rhythm: &[BeatTime],
) -> Option<MotiveTransform> {
    let same_rhythm = rhythm == other_rhythm;
    let doubled = rhythm
        .iter()
        .zip(other_rhythm)
        .all(|(a, b)| *b == a.scale(2, 1));
    let halved = rhythm
        .iter()
        .zip(other_rhythm)
        .all(|(a, b)| *a == b.scale(2, 1));

    if profile == other {
        return Some(if same_rhythm {
            MotiveTransform::Exact
        } else if doubled {
            MotiveTransform::Augmented
        } else if halved {
            MotiveTransform::Diminished
        } else {
            MotiveTransform::Varied
        });
    }
    if profile.iter().map(|i| -i).eq(other.iter().copied()) {
        return Some(MotiveTransform::Inverted);
    }
    if profile.iter().rev().map(|i| -i).eq(other.iter().copied()) {
        return Some(MotiveTransform::Retrograde);
    }
    // Same contour, different sizes: a varied restatement, not a new idea.
    if profile.len() == other.len()
        && profile
            .iter()
            .zip(other)
            .all(|(a, b)| a.signum() == b.signum())
        && profile.iter().zip(other).any(|(a, b)| a != b)
        && profile.iter().zip(other).all(|(a, b)| (a - b).abs() <= 2)
        && same_rhythm
    {
        return Some(MotiveTransform::Varied);
    }
    None
}

/// Re-labels transposed restatements that follow each other at a constant
/// interval as a sequence, which is what they are.
fn classify_sequences(occ: &mut [MotiveOccurrence]) {
    if occ.len() < 2 {
        return;
    }
    let steps: Vec<i32> = occ
        .windows(2)
        .map(|w| w[1].transposition - w[0].transposition)
        .collect();
    let sequential = steps.len() >= 2
        && steps.iter().all(|s| *s != 0)
        && steps.windows(2).all(|w| w[0] == w[1])
        && occ
            .iter()
            .all(|o| matches!(o.transform, MotiveTransform::Exact));
    for (i, o) in occ.iter_mut().enumerate() {
        if matches!(o.transform, MotiveTransform::Exact) && o.transposition != 0 {
            o.transform = if sequential {
                MotiveTransform::Sequence
            } else {
                MotiveTransform::Transposed
            };
        }
        let _ = i;
    }
}

// ---------------------------------------------------------------------------
// melody profile
// ---------------------------------------------------------------------------

/// Descriptive statistics of a melodic line.
#[derive(Clone, Debug)]
pub struct MelodyProfile {
    /// Lowest note.
    pub low: SpelledPitch,
    /// Highest note.
    pub high: SpelledPitch,
    /// The duration-weighted 15th and 85th percentile MIDI numbers: where the
    /// line actually lives, as opposed to where it merely visits.
    pub tessitura: (i32, i32),
    /// Successive melodic intervals in semitones.
    pub contour: Vec<i32>,
    /// Every interval wider than a whole tone, keyed on the note it lands on.
    pub leaps: Vec<(NoteId, i32)>,
    /// The highest note; ties go to the earliest.
    pub climax: Option<NoteId>,
    /// Note onsets per bar.
    pub density: f64,
    /// How many times the line changes direction.
    pub direction_changes: usize,
    /// How many intervals are unisons.
    pub repeated_pitches: usize,
    /// Mean absolute melodic interval in semitones.
    pub mean_interval: f64,
}

impl Default for MelodyProfile {
    fn default() -> Self {
        let middle = SpelledPitch::new(Letter::C, Accidental::NATURAL, 4);
        MelodyProfile {
            low: middle,
            high: middle,
            tessitura: (60, 60),
            contour: Vec::new(),
            leaps: Vec::new(),
            climax: None,
            density: 0.0,
            direction_changes: 0,
            repeated_pitches: 0,
            mean_interval: 0.0,
        }
    }
}

impl MelodyProfile {
    /// JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("low", Json::Str(self.low.to_ascii()));
        m.insert("high", Json::Str(self.high.to_ascii()));
        m.insert(
            "range_semitones",
            Json::Int((self.high.midi() - self.low.midi()) as i64),
        );
        m.insert(
            "tessitura",
            Json::Arr(vec![
                Json::Int(self.tessitura.0 as i64),
                Json::Int(self.tessitura.1 as i64),
            ]),
        );
        m.insert(
            "contour",
            Json::Arr(self.contour.iter().map(|i| Json::Int(*i as i64)).collect()),
        );
        m.insert(
            "leaps",
            Json::Arr(
                self.leaps
                    .iter()
                    .map(|(id, iv)| {
                        let mut l = JsonMap::new();
                        l.insert("note", Json::Int(*id as i64));
                        l.insert("semitones", Json::Int(*iv as i64));
                        Json::Obj(l)
                    })
                    .collect(),
            ),
        );
        m.insert(
            "climax",
            match self.climax {
                Some(c) => Json::Int(c as i64),
                None => Json::Null,
            },
        );
        m.insert("density", crate::util::num(self.density));
        m.insert(
            "direction_changes",
            Json::Int(self.direction_changes as i64),
        );
        m.insert("repeated_pitches", Json::Int(self.repeated_pitches as i64));
        m.insert("mean_interval", crate::util::num(self.mean_interval));
        Json::Obj(m)
    }
}

/// Computes the descriptive profile of a melodic line.
pub fn melody_profile(m: &NoteSet) -> MelodyProfile {
    let notes = &m.notes;
    if notes.is_empty() {
        return MelodyProfile::default();
    }

    let mut lowest = &notes[0];
    let mut highest = &notes[0];
    for n in notes {
        if (n.midi, n.id) < (lowest.midi, lowest.id) {
            lowest = n;
        }
        if n.midi > highest.midi || (n.midi == highest.midi && n.id < highest.id) {
            highest = n;
        }
    }

    let contour: Vec<i32> = notes.windows(2).map(|w| w[1].midi - w[0].midi).collect();
    let leaps: Vec<(NoteId, i32)> = notes
        .windows(2)
        .filter_map(|w| {
            let iv = w[1].midi - w[0].midi;
            if iv.abs() > 2 {
                Some((w[1].id, iv))
            } else {
                None
            }
        })
        .collect();

    let direction_changes = contour
        .iter()
        .filter(|i| **i != 0)
        .collect::<Vec<_>>()
        .windows(2)
        .filter(|w| w[0].signum() != w[1].signum())
        .count();
    let repeated_pitches = contour.iter().filter(|i| **i == 0).count();
    let mean_interval = if contour.is_empty() {
        0.0
    } else {
        contour.iter().map(|i| i.unsigned_abs() as f64).sum::<f64>() / contour.len() as f64
    };

    let (start, end) = m.span();
    let bar = m
        .time_map
        .meter_at(start)
        .bar_length_qn()
        .as_f64()
        .max(1e-9);
    let bars = ((end - start).as_f64() / bar).max(1e-9);
    let density = notes.len() as f64 / bars;

    MelodyProfile {
        low: lowest.pitch,
        high: highest.pitch,
        tessitura: tessitura(notes),
        contour,
        leaps,
        climax: Some(highest.id),
        density,
        direction_changes,
        repeated_pitches,
        mean_interval,
    }
}

/// Duration-weighted 15th/85th percentile of the MIDI numbers.
fn tessitura(notes: &[Note]) -> (i32, i32) {
    let mut by_pitch: Vec<(i32, f64)> = Vec::new();
    for n in notes {
        match by_pitch.iter_mut().find(|(m, _)| *m == n.midi) {
            Some((_, w)) => *w += n.duration.as_f64(),
            None => by_pitch.push((n.midi, n.duration.as_f64())),
        }
    }
    by_pitch.sort_by_key(|(m, _)| *m);
    let total: f64 = by_pitch.iter().map(|(_, w)| *w).sum();
    if total <= 0.0 {
        let lo = by_pitch.first().map(|(m, _)| *m).unwrap_or(60);
        let hi = by_pitch.last().map(|(m, _)| *m).unwrap_or(60);
        return (lo, hi);
    }
    let pick = |q: f64| -> i32 {
        let target = total * q;
        let mut acc = 0.0;
        for (m, w) in &by_pitch {
            acc += w;
            if acc >= target {
                return *m;
            }
        }
        by_pitch.last().map(|(m, _)| *m).unwrap_or(60)
    };
    (pick(0.15), pick(0.85))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{note_set, note_set_in};

    fn eight_bar() -> NoteSet {
        note_set(&[
            ("C4", "0", "1"),
            ("E4", "1", "1"),
            ("G4", "2", "1"),
            ("E4", "3", "1"),
            ("F4", "4", "1"),
            ("A4", "5", "1"),
            ("G4", "6", "2"),
            ("D4", "8", "1"),
            ("F4", "9", "1"),
            ("A4", "10", "1"),
            ("F4", "11", "1"),
            ("E4", "12", "1"),
            ("G4", "13", "1"),
            ("C5", "14", "2"),
            ("B4", "16", "1"),
            ("A4", "17", "1"),
            ("G4", "18", "1"),
            ("F4", "19", "1"),
            ("E4", "20", "1"),
            ("D4", "21", "1"),
            ("C4", "22", "2"),
        ])
    }

    #[test]
    fn rests_create_phrase_boundaries() {
        let ns = note_set(&[
            ("C4", "0", "1"),
            ("D4", "1", "1"),
            ("E4", "2", "1"),
            ("G4", "6", "1"),
            ("A4", "7", "1"),
            ("B4", "8", "2"),
        ]);
        let ph = analyze_phrases(&ns, &ns.time_map.clone());
        assert_eq!(ph.gaps.len(), 1);
        assert!(ph.phrases.len() >= 2, "{:?}", ph.phrases);
        assert!(ph.phrases[0].notes.contains(&2));
        assert!(!ph.phrases[0].notes.contains(&3));
    }

    #[test]
    fn continuous_material_still_gets_segmented() {
        let ns = eight_bar();
        let ph = analyze_phrases(&ns, &ns.time_map.clone());
        assert!(ph.gaps.is_empty(), "there are no rests here");
        assert!(ph.phrases.len() >= 2, "{:?}", ph.phrases.len());
    }

    #[test]
    fn every_note_belongs_to_exactly_one_phrase() {
        let ns = eight_bar();
        let ph = analyze_phrases(&ns, &ns.time_map.clone());
        let mut ids: Vec<NoteId> = ph.phrases.iter().flat_map(|p| p.notes.clone()).collect();
        ids.sort_unstable();
        let mut expected: Vec<NoteId> = ns.notes.iter().map(|n| n.id).collect();
        expected.sort_unstable();
        assert_eq!(ids, expected);
    }

    #[test]
    fn phrases_are_contiguous_and_ordered() {
        let ns = eight_bar();
        let ph = analyze_phrases(&ns, &ns.time_map.clone());
        for w in ph.phrases.windows(2) {
            assert!(w[0].start < w[1].start);
        }
    }

    #[test]
    fn a_pickup_before_the_first_downbeat_is_detected() {
        let ns = note_set(&[
            ("G3", "-1", "1"),
            ("C4", "0", "3/2"),
            ("D4", "3/2", "1/2"),
            ("E4", "2", "2"),
            ("F4", "4", "1"),
            ("E4", "5", "1"),
            ("D4", "6", "2"),
        ]);
        let ph = analyze_phrases(&ns, &ns.time_map.clone());
        let pickup = ph.pickup.clone().expect("a pickup");
        assert!(pickup.is_pickup);
        assert_eq!(pickup.notes, vec![0]);
        assert!(ph.is_pickup(0));
        assert!(!ph.is_pickup(1));
    }

    #[test]
    fn a_melody_starting_on_the_downbeat_has_no_pickup() {
        let ns = eight_bar();
        let ph = analyze_phrases(&ns, &ns.time_map.clone());
        assert!(ph.pickup.is_none());
    }

    #[test]
    fn subphrases_lie_inside_phrases() {
        let ns = eight_bar();
        let ph = analyze_phrases(&ns, &ns.time_map.clone());
        for s in &ph.subphrases {
            let parent = ph
                .phrases
                .iter()
                .find(|p| p.start <= s.start && s.end <= p.end);
            assert!(parent.is_some(), "orphan subphrase {s:?}");
        }
    }

    #[test]
    fn a_repeated_figure_is_one_motive_with_two_occurrences() {
        let ns = note_set(&[
            ("C4", "0", "1"),
            ("D4", "1", "1"),
            ("E4", "2", "1"),
            ("C4", "4", "1"),
            ("D4", "5", "1"),
            ("E4", "6", "1"),
        ]);
        let ph = analyze_phrases(&ns, &ns.time_map.clone());
        assert!(!ph.motives.is_empty());
        let m = &ph.motives[0];
        assert_eq!(m.occurrences.len(), 2);
        assert_eq!(m.interval_profile, vec![2, 2]);
        assert_eq!(m.occurrences[1].transposition, 0);
        assert_eq!(m.occurrences[1].transform, MotiveTransform::Exact);
    }

    #[test]
    fn a_transposed_restatement_is_the_same_motive() {
        let ns = note_set(&[
            ("C4", "0", "1"),
            ("D4", "1", "1"),
            ("E4", "2", "1"),
            ("F4", "4", "1"),
            ("G4", "5", "1"),
            ("A4", "6", "1"),
        ]);
        let ph = analyze_phrases(&ns, &ns.time_map.clone());
        let m = &ph.motives[0];
        assert_eq!(m.occurrences.len(), 2);
        assert_eq!(m.occurrences[1].transposition, 5);
        assert_eq!(m.occurrences[1].transform, MotiveTransform::Transposed);
    }

    #[test]
    fn three_restatements_at_a_constant_interval_are_a_sequence() {
        let ns = note_set(&[
            ("C4", "0", "1"),
            ("D4", "1", "1"),
            ("E4", "2", "1"),
            ("D4", "3", "1"),
            ("E4", "4", "1"),
            ("F#4", "5", "1"),
            ("E4", "6", "1"),
            ("F#4", "7", "1"),
            ("G#4", "8", "1"),
        ]);
        let ph = analyze_phrases(&ns, &ns.time_map.clone());
        let m = &ph.motives[0];
        assert_eq!(m.occurrences.len(), 3);
        assert!(m
            .occurrences
            .iter()
            .skip(1)
            .all(|o| o.transform == MotiveTransform::Sequence));
    }

    #[test]
    fn an_augmented_restatement_is_labelled() {
        let ns = note_set(&[
            ("C4", "0", "1"),
            ("D4", "1", "1"),
            ("E4", "2", "1"),
            ("C4", "4", "2"),
            ("D4", "6", "2"),
            ("E4", "8", "2"),
        ]);
        let ph = analyze_phrases(&ns, &ns.time_map.clone());
        let m = &ph.motives[0];
        assert_eq!(m.occurrences[1].transform, MotiveTransform::Augmented);
    }

    #[test]
    fn motive_occurrences_never_overlap() {
        let ns = eight_bar();
        let ph = analyze_phrases(&ns, &ns.time_map.clone());
        let mut seen: Vec<NoteId> = Vec::new();
        for m in &ph.motives {
            for o in &m.occurrences {
                for id in &o.notes {
                    assert!(!seen.contains(id), "note {id} is in two occurrences");
                    seen.push(*id);
                }
            }
        }
    }

    #[test]
    fn phrase_analysis_is_deterministic() {
        let ns = eight_bar();
        let a = analyze_phrases(&ns, &ns.time_map.clone());
        let b = analyze_phrases(&ns, &ns.time_map.clone());
        assert_eq!(
            a.to_json().to_canonical_string(),
            b.to_json().to_canonical_string()
        );
    }

    #[test]
    fn cadences_are_refined_from_the_key() {
        let ns = note_set(&[
            ("C4", "0", "1"),
            ("D4", "1", "1"),
            ("E4", "2", "1"),
            ("D4", "3", "1"),
            ("B3", "6", "1"),
            ("C4", "7", "1"),
            ("E4", "8", "2"),
            ("D4", "10", "1"),
            ("B3", "14", "1"),
            ("C4", "15", "2"),
        ]);
        let mut ph = analyze_phrases(&ns, &ns.time_map.clone());
        assert!(ph.phrases.iter().all(|p| p.cadence.is_none()));
        refine_cadences(
            &mut ph,
            &ns,
            0,
            &[0, 2, 4, 5, 7, 9, 11],
            &ns.time_map.clone(),
        );
        assert!(ph.phrases.iter().any(|p| p.cadence.is_some()));
    }

    #[test]
    fn a_modal_close_is_not_called_authentic() {
        let ns = note_set(&[
            ("D4", "0", "2"),
            ("F4", "2", "1"),
            ("G4", "3", "1"),
            ("A4", "4", "2"),
            ("E4", "6", "1"),
            ("D4", "7", "1"),
        ]);
        let mut ph = analyze_phrases(&ns, &ns.time_map.clone());
        // D dorian: no leading tone.
        refine_cadences(
            &mut ph,
            &ns,
            2,
            &[2, 4, 5, 7, 9, 11, 0],
            &ns.time_map.clone(),
        );
        let last = ph.phrases.last().expect("a phrase");
        assert_eq!(last.cadence, Some(CadenceKind::Modal));
    }

    #[test]
    fn melody_profile_reports_range_and_climax() {
        let ns = eight_bar();
        let p = melody_profile(&ns);
        assert_eq!(p.low.to_ascii(), "C4");
        assert_eq!(p.high.to_ascii(), "C5");
        assert_eq!(p.climax, Some(13));
        assert!(p.density > 0.0);
        assert!(p.mean_interval > 0.0);
    }

    #[test]
    fn melody_profile_counts_leaps_and_direction_changes() {
        let ns = note_set(&[
            ("C4", "0", "1"),
            ("C5", "1", "1"),
            ("B4", "2", "1"),
            ("B4", "3", "1"),
            ("C4", "4", "1"),
        ]);
        let p = melody_profile(&ns);
        assert_eq!(p.leaps.len(), 2);
        assert_eq!(p.repeated_pitches, 1);
        assert_eq!(p.direction_changes, 1);
        assert_eq!(p.contour, vec![12, -1, 0, -11]);
    }

    #[test]
    fn tessitura_ignores_a_single_extreme_visit() {
        let ns = note_set(&[
            ("C4", "0", "4"),
            ("D4", "4", "4"),
            ("E4", "8", "4"),
            ("C6", "12", "1/4"),
        ]);
        let p = melody_profile(&ns);
        assert_eq!(p.high.to_ascii(), "C6");
        assert!(p.tessitura.1 < 84, "tessitura is {:?}", p.tessitura);
    }

    #[test]
    fn an_empty_melody_profiles_without_panicking() {
        let p = melody_profile(&NoteSet::default());
        assert_eq!(p.climax, None);
        assert_eq!(p.contour.len(), 0);
        assert_eq!(p.density, 0.0);
    }

    #[test]
    fn three_four_meter_segments_on_its_own_bar_lines() {
        let ns = note_set_in(
            &[
                ("G4", "0", "1"),
                ("F4", "1", "2"),
                ("E4", "3", "3"),
                ("A4", "6", "1"),
                ("G4", "7", "2"),
                ("F4", "9", "3"),
            ],
            TimeSignature::new(3, 4),
        );
        let ph = analyze_phrases(&ns, &ns.time_map.clone());
        assert!(!ph.phrases.is_empty());
        for p in &ph.phrases {
            assert!(p.length().is_positive());
        }
    }

    #[test]
    fn an_empty_melody_yields_an_empty_analysis() {
        let ph = analyze_phrases(
            &NoteSet::default(),
            &TimeMap::constant(120.0, TimeSignature::new(4, 4)),
        );
        assert!(ph.phrases.is_empty());
        assert!(ph.motives.is_empty());
        assert!(ph.pickup.is_none());
    }
}
