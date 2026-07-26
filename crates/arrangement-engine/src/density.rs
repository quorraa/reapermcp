//! Measuring what was written, and controlling it.
//!
//! Density, register, note length, dynamics and articulation are the levers the
//! brief names; this module turns each of them into a number that can be
//! compared, so "the density control changed the output" is a measurement
//! rather than a claim. [`Contrast`] is the same idea applied between two
//! sections, and it is what `arrangement.contrast_beyond_dynamics` is fed.

use music_domain::prelude::*;
use std::collections::BTreeSet;

/// Everything measurable about one part over one span.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PartMetrics {
    /// Notes written.
    pub notes: usize,
    /// Distinct attack instants.
    pub onsets: usize,
    /// Attacks per quarter note.
    pub onsets_per_qn: f64,
    /// Lowest and highest sounding MIDI number, if any.
    pub register: Option<(i32, i32)>,
    /// Mean sounding MIDI number.
    pub mean_midi: f64,
    /// Mean note length in quarter notes.
    pub mean_duration: f64,
    /// Share of the span with nothing sounding, `0.0..=1.0`.
    pub rest_ratio: f64,
    /// Mean velocity.
    pub mean_velocity: f64,
    /// Greatest number of notes sounding at once.
    pub max_polyphony: usize,
    /// Articulation labels used, sorted and deduplicated.
    pub articulations: Vec<String>,
}

impl PartMetrics {
    /// JSON form, so a plan can carry its own measurements.
    pub fn to_json(&self) -> qjson::Json {
        qjson::json_obj! {
            "notes" => self.notes as i64,
            "onsets" => self.onsets as i64,
            "onsets_per_qn" => self.onsets_per_qn,
            "low_midi" => match self.register { Some((lo, _)) => qjson::Json::Int(i64::from(lo)), None => qjson::Json::Null },
            "high_midi" => match self.register { Some((_, hi)) => qjson::Json::Int(i64::from(hi)), None => qjson::Json::Null },
            "mean_midi" => self.mean_midi,
            "mean_duration_qn" => self.mean_duration,
            "rest_ratio" => self.rest_ratio,
            "mean_velocity" => self.mean_velocity,
            "max_polyphony" => self.max_polyphony as i64,
            "articulations" => qjson::Json::Arr(
                self.articulations.iter().map(|a| qjson::Json::Str(a.clone())).collect()
            ),
        }
    }
}

/// Measures a note list over a span.
pub fn measure(notes: &[Note], span: (BeatTime, BeatTime)) -> PartMetrics {
    let length = (span.1 - span.0).as_f64().max(f64::MIN_POSITIVE);
    let mut metrics = PartMetrics {
        notes: notes.len(),
        ..PartMetrics::default()
    };
    if notes.is_empty() {
        metrics.rest_ratio = 1.0;
        return metrics;
    }
    let mut onsets: BTreeSet<(i64, i64)> = BTreeSet::new();
    let mut articulations: BTreeSet<String> = BTreeSet::new();
    let mut lo = notes[0].midi;
    let mut hi = notes[0].midi;
    let mut midi_sum = 0.0;
    let mut duration_sum = 0.0;
    let mut velocity_sum = 0.0;
    for n in notes {
        onsets.insert((n.onset.num(), n.onset.den()));
        lo = lo.min(n.midi);
        hi = hi.max(n.midi);
        midi_sum += f64::from(n.midi);
        duration_sum += n.duration.as_f64();
        velocity_sum += f64::from(n.velocity);
        if let Some(a) = &n.articulation {
            articulations.insert(a.clone());
        }
    }
    metrics.onsets = onsets.len();
    metrics.onsets_per_qn = onsets.len() as f64 / length;
    metrics.register = Some((lo, hi));
    metrics.mean_midi = midi_sum / notes.len() as f64;
    metrics.mean_duration = duration_sum / notes.len() as f64;
    metrics.mean_velocity = velocity_sum / notes.len() as f64;
    metrics.rest_ratio = rest_ratio(notes, span);
    metrics.max_polyphony = max_polyphony(notes);
    metrics.articulations = articulations.into_iter().collect();
    metrics
}

/// The share of a span during which nothing is sounding.
pub fn rest_ratio(notes: &[Note], span: (BeatTime, BeatTime)) -> f64 {
    let total = (span.1 - span.0).as_f64();
    if total <= 0.0 {
        return 0.0;
    }
    let mut intervals: Vec<(BeatTime, BeatTime)> = notes
        .iter()
        .map(|n| (n.onset.max(span.0), n.end().min(span.1)))
        .filter(|(s, e)| e > s)
        .collect();
    intervals.sort();
    let mut covered = 0.0;
    let mut cursor = span.0;
    for (s, e) in intervals {
        if e <= cursor {
            continue;
        }
        let from = s.max(cursor);
        covered += (e - from).as_f64();
        cursor = e;
    }
    (1.0 - covered / total).clamp(0.0, 1.0)
}

/// The greatest number of notes sounding simultaneously.
pub fn max_polyphony(notes: &[Note]) -> usize {
    let mut best = 0usize;
    for a in notes {
        let count = notes.iter().filter(|b| b.overlaps(a) || b.id == a.id).count();
        best = best.max(count);
    }
    best
}

/// Guarantees at least one audible silence inside every window.
///
/// `arrangement.phrases_need_rests` is a rule with an exception for sustained
/// textures, so the caller decides whether to apply this at all; when it does,
/// the shortening falls on the *last* note of the window, which is where a
/// player would breathe. Returns how many windows were altered.
pub fn ensure_rest(
    notes: &mut [Note],
    windows: &[(BeatTime, BeatTime)],
    minimum: BeatTime,
) -> usize {
    let mut changed = 0usize;
    for (start, end) in windows {
        if *end <= *start {
            continue;
        }
        let span = (*start, *end);
        if rest_ratio(notes, span) * (span.1 - span.0).as_f64() >= minimum.as_f64() {
            continue;
        }
        // The last note that ends inside the window is the one that gives way.
        let mut chosen: Option<usize> = None;
        for (i, n) in notes.iter().enumerate() {
            if n.onset >= *start && n.onset < *end {
                match chosen {
                    Some(c) if notes[c].onset >= n.onset => {}
                    _ => chosen = Some(i),
                }
            }
        }
        let Some(i) = chosen else { continue };
        let note = &mut notes[i];
        let available = (*end).min(note.end()) - note.onset;
        if available <= minimum {
            continue;
        }
        let wanted = available - minimum;
        if wanted.is_positive() && wanted < note.duration {
            note.duration = wanted;
            changed += 1;
        }
    }
    changed
}

/// Drops notes whose onsets a higher-priority part already occupies.
///
/// Onset-density control, applied after the fact: the part keeps its shape but
/// stops articulating in unison with whoever outranks it. Returns how many
/// notes were removed.
pub fn thin_against(notes: &mut Vec<Note>, busy: &[BeatTime], keep_at_least: usize) -> usize {
    if busy.is_empty() {
        return 0;
    }
    let before = notes.len();
    let mut kept: Vec<Note> = Vec::with_capacity(before);
    for n in notes.drain(..) {
        if busy.contains(&n.onset) && kept.len() + 1 > keep_at_least {
            continue;
        }
        kept.push(n);
    }
    if kept.len() < keep_at_least.min(before) {
        // Never silence a part entirely in the name of clarity.
        return 0;
    }
    let removed = before - kept.len();
    *notes = kept;
    removed
}

/// Shortens every note by a factor, keeping durations strictly positive.
pub fn shorten(notes: &mut [Note], scale: f64) {
    if scale >= 1.0 {
        return;
    }
    for n in notes.iter_mut() {
        n.duration = crate::patterns::scale_length(n.duration, scale);
    }
}

/// Moves every note by whole octaves, staying inside a window.
///
/// Register separation has to preserve the harmony, which is exactly what an
/// octave transposition does.
pub fn transpose_octaves(notes: &mut [Note], octaves: i32, window: (i32, i32)) -> usize {
    if octaves == 0 {
        return 0;
    }
    let mut moved = 0usize;
    for n in notes.iter_mut() {
        let target = n.midi + 12 * octaves;
        if target >= window.0 && target <= window.1 && (0..=127).contains(&target) {
            let letter = n.pitch.letter;
            let accidental = n.pitch.accidental;
            n.midi = target;
            n.pitch = SpelledPitch::new(letter, accidental, n.pitch.octave + octaves);
            moved += 1;
        }
    }
    moved
}

/// The measurable difference between two parts or sections.
///
/// The brief's demand that contrast use more than volume is checked here:
/// [`Contrast::is_dynamics_only`] is true only when velocity is the sole thing
/// that moved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Contrast {
    /// Difference in mean register, semitones.
    pub register: f64,
    /// Difference in attacks per quarter note.
    pub density: f64,
    /// Difference in mean note length, quarter notes.
    pub note_length: f64,
    /// Difference in mean velocity.
    pub velocity: f64,
    /// Difference in the number of sounding layers.
    pub layers: i64,
    /// True when the two use different articulation vocabularies.
    pub articulation_differs: bool,
    /// True when the two use different textures (pattern ids).
    pub texture_differs: bool,
    /// Difference in silence, `0.0..=1.0`.
    pub silence: f64,
}

impl Contrast {
    /// How many non-dynamic dimensions moved measurably.
    pub fn non_dynamic_dimensions(&self) -> usize {
        let mut n = 0;
        if self.register.abs() >= 1.0 {
            n += 1;
        }
        if self.density.abs() >= 0.05 {
            n += 1;
        }
        if self.note_length.abs() >= 0.1 {
            n += 1;
        }
        if self.layers != 0 {
            n += 1;
        }
        if self.articulation_differs {
            n += 1;
        }
        if self.texture_differs {
            n += 1;
        }
        if self.silence.abs() >= 0.05 {
            n += 1;
        }
        n
    }

    /// True when the only thing that changed is how loud it is.
    pub fn is_dynamics_only(&self) -> bool {
        self.velocity.abs() >= 1.0 && self.non_dynamic_dimensions() == 0
    }

    /// True when the two are indistinguishable by any measure.
    pub fn is_identical(&self) -> bool {
        self.velocity.abs() < 1.0 && self.non_dynamic_dimensions() == 0
    }

    /// JSON form.
    pub fn to_json(&self) -> qjson::Json {
        qjson::json_obj! {
            "register_semitones" => self.register,
            "density" => self.density,
            "note_length_qn" => self.note_length,
            "velocity" => self.velocity,
            "layers" => self.layers,
            "articulation_differs" => self.articulation_differs,
            "texture_differs" => self.texture_differs,
            "silence" => self.silence,
            "non_dynamic_dimensions" => self.non_dynamic_dimensions() as i64,
        }
    }
}

/// Compares two measurements.
pub fn contrast(a: &PartMetrics, b: &PartMetrics) -> Contrast {
    Contrast {
        register: b.mean_midi - a.mean_midi,
        density: b.onsets_per_qn - a.onsets_per_qn,
        note_length: b.mean_duration - a.mean_duration,
        velocity: b.mean_velocity - a.mean_velocity,
        layers: b.max_polyphony as i64 - a.max_polyphony as i64,
        articulation_differs: a.articulations != b.articulations,
        texture_differs: false,
        silence: b.rest_ratio - a.rest_ratio,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: NoteId, midi: i32, onset: (i64, i64), duration: (i64, i64)) -> Note {
        let mut n = Note::new(
            id,
            SpelledPitch::from_midi(midi, None),
            BeatTime::new(onset.0, onset.1),
            BeatTime::new(duration.0, duration.1),
        );
        n.midi = midi;
        n
    }

    fn span() -> (BeatTime, BeatTime) {
        (BeatTime::ZERO, BeatTime::from_quarters(4))
    }

    #[test]
    fn an_empty_part_is_all_rest() {
        let m = measure(&[], span());
        assert_eq!(m.notes, 0);
        assert_eq!(m.rest_ratio, 1.0);
        assert_eq!(m.register, None);
    }

    #[test]
    fn a_continuous_part_has_no_rest() {
        let notes = vec![
            note(0, 60, (0, 1), (2, 1)),
            note(1, 62, (2, 1), (2, 1)),
        ];
        let m = measure(&notes, span());
        assert_eq!(m.rest_ratio, 0.0);
        assert_eq!(m.onsets, 2);
        assert_eq!(m.onsets_per_qn, 0.5);
        assert_eq!(m.register, Some((60, 62)));
        assert_eq!(m.max_polyphony, 1);
    }

    #[test]
    fn overlapping_notes_raise_polyphony() {
        let notes = vec![
            note(0, 60, (0, 1), (4, 1)),
            note(1, 64, (0, 1), (4, 1)),
            note(2, 67, (0, 1), (4, 1)),
        ];
        assert_eq!(max_polyphony(&notes), 3);
        assert_eq!(measure(&notes, span()).onsets, 1);
    }

    #[test]
    fn rest_ratio_ignores_overlap_double_counting() {
        let notes = vec![
            note(0, 60, (0, 1), (2, 1)),
            note(1, 64, (0, 1), (2, 1)),
        ];
        assert!((rest_ratio(&notes, span()) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn ensure_rest_shortens_the_last_note() {
        let mut notes = vec![
            note(0, 60, (0, 1), (2, 1)),
            note(1, 62, (2, 1), (2, 1)),
        ];
        let changed = ensure_rest(&mut notes, &[span()], BeatTime::new(1, 2));
        assert_eq!(changed, 1);
        assert!(rest_ratio(&notes, span()) > 0.0);
        assert!(notes.iter().all(|n| n.duration.is_positive()));
    }

    #[test]
    fn ensure_rest_leaves_a_part_that_already_breathes() {
        let mut notes = vec![note(0, 60, (0, 1), (1, 1))];
        assert_eq!(ensure_rest(&mut notes, &[span()], BeatTime::new(1, 2)), 0);
    }

    #[test]
    fn thinning_never_empties_a_part() {
        let mut notes = vec![note(0, 60, (0, 1), (1, 1)), note(1, 62, (1, 1), (1, 1))];
        let busy = vec![BeatTime::ZERO, BeatTime::from_quarters(1)];
        assert_eq!(thin_against(&mut notes, &busy, 2), 0);
        assert_eq!(notes.len(), 2);
    }

    #[test]
    fn thinning_removes_colliding_onsets() {
        let mut notes = vec![
            note(0, 60, (0, 1), (1, 1)),
            note(1, 62, (1, 1), (1, 1)),
            note(2, 64, (2, 1), (1, 1)),
        ];
        let removed = thin_against(&mut notes, &[BeatTime::from_quarters(1)], 1);
        assert_eq!(removed, 1);
        assert_eq!(notes.len(), 2);
    }

    #[test]
    fn octave_transposition_stays_in_the_window() {
        let mut notes = vec![note(0, 60, (0, 1), (1, 1))];
        assert_eq!(transpose_octaves(&mut notes, 1, (48, 84)), 1);
        assert_eq!(notes[0].midi, 72);
        assert_eq!(notes[0].pitch.midi(), 72);
        assert_eq!(transpose_octaves(&mut notes, 2, (48, 84)), 0);
        assert_eq!(notes[0].midi, 72);
    }

    #[test]
    fn shortening_keeps_durations_positive() {
        let mut notes = vec![note(0, 60, (0, 1), (1, 8))];
        shorten(&mut notes, 0.1);
        assert!(notes[0].duration.is_positive());
    }

    #[test]
    fn dynamics_only_contrast_is_detected() {
        let a = PartMetrics {
            mean_velocity: 70.0,
            ..PartMetrics::default()
        };
        let b = PartMetrics {
            mean_velocity: 110.0,
            ..PartMetrics::default()
        };
        let c = contrast(&a, &b);
        assert!(c.is_dynamics_only());
        assert_eq!(c.non_dynamic_dimensions(), 0);
    }

    #[test]
    fn real_contrast_is_not_dynamics_only() {
        let a = PartMetrics {
            mean_velocity: 70.0,
            mean_midi: 60.0,
            onsets_per_qn: 0.25,
            ..PartMetrics::default()
        };
        let b = PartMetrics {
            mean_velocity: 110.0,
            mean_midi: 72.0,
            onsets_per_qn: 1.0,
            ..PartMetrics::default()
        };
        let c = contrast(&a, &b);
        assert!(!c.is_dynamics_only());
        assert!(c.non_dynamic_dimensions() >= 2);
        assert!(c.to_json().get("register_semitones").is_some());
    }

    #[test]
    fn identical_metrics_show_no_contrast() {
        let a = PartMetrics::default();
        assert!(contrast(&a, &a).is_identical());
    }
}
