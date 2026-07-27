//! Stage 4 — harmonic-grid selection.
//!
//! The single most damaging thing a naive harmoniser does is put a chord under
//! every note. It turns a scale run into a chord sequence nobody wrote, and it
//! makes every later stage — voice leading, arrangement, looping — solve the
//! wrong problem. So the grid is chosen **before** any chord is, from the
//! structure of the melody rather than from its note list:
//!
//! * the profile's `harmonic_rhythm` block sets the starting resolution and the
//!   hard floor and ceiling on a slot's length;
//! * phrase boundaries force a slot boundary, because harmony changes where the
//!   music breathes;
//! * a slot with no structural note in it is merged into its predecessor, so a
//!   run of decorative notes gets one chord, not six;
//! * a slot is only split when two structural notes inside it genuinely
//!   disagree, and never below the profile's floor.
//!
//! A short note on a weak beat therefore cannot create a chord change: it never
//! becomes structural, so it never forces a boundary and it never blocks a
//! merge. That is the behaviour `melody.short_weak_note_does_not_force_chord_change`
//! exists to protect.

use crate::phrase::PhraseAnalysis;
use crate::salience::SalienceReport;
use crate::util::{cmp_f64, num};
use music_domain::prelude::*;
use qjson::{Json, JsonMap};
use theory_kb::ResolvedProfile;

/// How the harmonic grid is chosen.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum GridMode {
    /// Phrase-sensitive automatic rhythm.
    #[default]
    Auto,
    /// The chord rhythm the material itself implies.
    Existing,
    /// A fixed number of bars per chord (`0.5` is two chords per bar).
    Bars(f64),
    /// A fixed number of beats per chord.
    Beats(f64),
}

impl GridMode {
    /// Stable identifier.
    pub fn id(&self) -> &'static str {
        match self {
            GridMode::Auto => "auto",
            GridMode::Existing => "existing",
            GridMode::Bars(_) => "bars",
            GridMode::Beats(_) => "beats",
        }
    }

    /// Reads the wire form, e.g. `"auto"`, `"bars:1"`, `"beats:2"`.
    pub fn parse(s: &str) -> Option<GridMode> {
        match s.split_once(':') {
            Some(("bars", v)) => v.parse().ok().map(GridMode::Bars),
            Some(("beats", v)) => v.parse().ok().map(GridMode::Beats),
            Some(_) => None,
            None => match s {
                "auto" => Some(GridMode::Auto),
                "existing" => Some(GridMode::Existing),
                _ => None,
            },
        }
    }

    /// JSON form, used in the analysis-id derivation.
    pub fn to_json(&self) -> Json {
        match self {
            GridMode::Bars(v) => Json::Str(format!("bars:{}", crate::util::round6(*v))),
            GridMode::Beats(v) => Json::Str(format!("beats:{}", crate::util::round6(*v))),
            other => Json::Str(other.id().to_string()),
        }
    }
}

/// One harmonic slot.
#[derive(Clone, Debug, PartialEq)]
pub struct GridSlot {
    /// Slot start.
    pub start: BeatTime,
    /// Slot end.
    pub end: BeatTime,
    /// Melody notes whose onset falls inside the slot.
    pub melody_notes: Vec<NoteId>,
    /// The subset of those the harmony must agree with.
    pub structural_notes: Vec<NoteId>,
    /// True when a phrase closes in this slot.
    pub is_cadential: bool,
    /// Relative importance, `0.0..=1.0`.
    pub weight: f64,
}

impl GridSlot {
    /// Slot length.
    pub fn length(&self) -> BeatTime {
        self.end - self.start
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("start", self.start.to_json());
        m.insert("end", self.end.to_json());
        m.insert("melody_notes", crate::util::id_array(&self.melody_notes));
        m.insert(
            "structural_notes",
            crate::util::id_array(&self.structural_notes),
        );
        m.insert("is_cadential", Json::Bool(self.is_cadential));
        m.insert("weight", num(self.weight));
        Json::Obj(m)
    }
}

/// The result of stage 4.
#[derive(Clone, Debug)]
pub struct HarmonicGrid {
    /// Slots in time order, contiguous and non-overlapping.
    pub slots: Vec<GridSlot>,
    /// The mode actually used.
    pub mode_used: GridMode,
    /// Why the grid looks the way it does, in the user's language.
    pub rationale: String,
}

impl Default for HarmonicGrid {
    fn default() -> Self {
        HarmonicGrid {
            slots: Vec::new(),
            mode_used: GridMode::Auto,
            rationale: "no material to place harmony under".to_string(),
        }
    }
}

impl HarmonicGrid {
    /// The slot containing `qn`, if any.
    pub fn slot_at(&self, qn: BeatTime) -> Option<&GridSlot> {
        self.slots.iter().find(|s| s.start <= qn && qn < s.end)
    }

    /// True when a slot boundary falls exactly on `qn`.
    pub fn has_boundary_at(&self, qn: BeatTime) -> bool {
        self.slots.iter().any(|s| s.start == qn)
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("mode_used", self.mode_used.to_json());
        m.insert("slot_count", Json::Int(self.slots.len() as i64));
        m.insert("rationale", Json::Str(self.rationale.clone()));
        m.insert(
            "slots",
            Json::Arr(self.slots.iter().map(GridSlot::to_json).collect()),
        );
        Json::Obj(m)
    }
}

/// Runs stage 4.
///
/// The grid never degenerates to one slot per note: in `auto` mode the slot
/// count is bounded by the phrase structure and the profile's harmonic-rhythm
/// floor, and in every mode the boundaries come from the metrical grid rather
/// than from note onsets.
pub fn build_grid(
    m: &NoteSet,
    tm: &TimeMap,
    ph: &PhraseAnalysis,
    sal: &SalienceReport,
    mode: GridMode,
    profile: &ResolvedProfile,
) -> HarmonicGrid {
    if m.notes.is_empty() {
        return HarmonicGrid {
            slots: Vec::new(),
            mode_used: mode,
            rationale: "the melody is empty, so no harmonic grid was built".to_string(),
        };
    }
    let (start, end) = m.span();
    let bar = tm.meter_at(start).bar_length_qn();
    let beat = tm.meter_at(start).beat_unit_qn();
    let (min_slot, max_slot) = profile_limits(profile, bar);

    let (mut boundaries, mode_used, mut rationale) = match &mode {
        GridMode::Bars(n) => {
            let step = scale_by(bar, *n);
            (
                even_grid(start, end, step, tm),
                GridMode::Bars(*n),
                format!(
                    "fixed grid of {n} bar(s) per chord, as requested ({} quarter notes per slot)",
                    step.to_display()
                ),
            )
        }
        GridMode::Beats(n) => {
            let step = scale_by(beat, *n);
            (
                even_grid(start, end, step, tm),
                GridMode::Beats(*n),
                format!(
                    "fixed grid of {n} beat(s) per chord, as requested ({} quarter notes per slot)",
                    step.to_display()
                ),
            )
        }
        GridMode::Existing => {
            let (step, evidence) = detect_rhythm(m, tm, sal, bar, min_slot);
            (
                even_grid(start, end, step, tm),
                GridMode::Existing,
                format!(
                    "the material's own harmonic rhythm was detected as {} quarter notes per slot \
                     ({evidence})",
                    step.to_display()
                ),
            )
        }
        GridMode::Auto => {
            let slots_per_bar = profile
                .field_f64("harmonic_rhythm.default_slots_per_bar")
                .unwrap_or(1.0)
                .max(1.0);
            let step = clamp_len(scale_by(bar, 1.0 / slots_per_bar), min_slot, max_slot);
            (
                even_grid(start, end, step, tm),
                GridMode::Auto,
                format!(
                    "the {} profile prefers {} chord(s) per bar, giving a base slot of {} quarter \
                     notes",
                    profile.id,
                    slots_per_bar as i64,
                    step.to_display()
                ),
            )
        }
    };

    // Phrase starts are harmonic events in every mode: harmony changes where
    // the music breathes.
    let mut forced: Vec<BeatTime> = ph
        .phrases
        .iter()
        .filter(|p| !p.is_pickup)
        .map(|p| p.start)
        .collect();
    forced.retain(|b| *b > start && *b < end);
    for f in forced {
        if !boundaries.contains(&f) {
            boundaries.push(f);
        }
    }
    boundaries.sort();
    boundaries.dedup();

    if mode_used == GridMode::Auto {
        let before = boundaries.len();
        boundaries = merge_empty(boundaries, end, sal, m, min_slot, max_slot);
        boundaries = split_disagreements(boundaries, end, sal, m, min_slot);
        let after = boundaries.len();
        if after < before {
            rationale.push_str(&format!(
                "; {} slot(s) were merged because the melody gave no reason to change harmony",
                before - after
            ));
        } else if after > before {
            rationale.push_str(&format!(
                "; {} slot(s) were split because two structural notes disagreed",
                after - before
            ));
        }
        rationale.push_str("; phrase boundaries were kept as harmonic boundaries");
    }

    let slots = materialise(&boundaries, end, m, sal, ph);
    HarmonicGrid {
        slots,
        mode_used,
        rationale,
    }
}

/// The slot-length floor and ceiling the profile declares.
fn profile_limits(profile: &ResolvedProfile, bar: BeatTime) -> (BeatTime, BeatTime) {
    let parse = |key: &str, fallback: BeatTime| -> BeatTime {
        profile
            .field_str(key)
            .and_then(|s| BeatTime::parse(s).ok())
            .or_else(|| profile.field_f64(key).map(BeatTime::from_f64))
            .filter(|b| b.is_positive())
            .unwrap_or(fallback)
    };
    let min = parse("harmonic_rhythm.min_slot_qn", bar.scale(1, 2));
    let max = parse("harmonic_rhythm.max_slot_qn", bar * 4);
    if min > max {
        (max, min)
    } else {
        (min, max)
    }
}

/// `base * factor`, snapped to the rational grid.
fn scale_by(base: BeatTime, factor: f64) -> BeatTime {
    let v = BeatTime::from_f64(base.as_f64() * factor.max(1e-6));
    if v.is_positive() {
        v
    } else {
        base
    }
}

/// Clamps a slot length into `min..=max`.
fn clamp_len(v: BeatTime, min: BeatTime, max: BeatTime) -> BeatTime {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

/// Boundaries on an even grid aligned to the bar line at or before `start`.
fn even_grid(start: BeatTime, end: BeatTime, step: BeatTime, tm: &TimeMap) -> Vec<BeatTime> {
    let mut out = Vec::new();
    if !step.is_positive() {
        out.push(start);
        return out;
    }
    let anchor = tm.bar_start(tm.bar_of(start));
    let mut at = anchor;
    let mut guard = 0;
    while at < end && guard < 8192 {
        guard += 1;
        if at >= start {
            out.push(at);
        } else if at + step > start {
            out.push(start);
        }
        at = at + step;
    }
    if out.is_empty() {
        out.push(start);
    }
    out.sort();
    out.dedup();
    out
}

/// Detects the harmonic rhythm the material itself implies.
///
/// One, two or four chords per bar are tried, and the resolution whose
/// boundaries best coincide with the structural notes wins — with a penalty
/// for over-segmentation, so a busy melody does not imply a chord every beat.
fn detect_rhythm(
    m: &NoteSet,
    tm: &TimeMap,
    sal: &SalienceReport,
    bar: BeatTime,
    min_slot: BeatTime,
) -> (BeatTime, String) {
    let (start, end) = m.span();
    let mut best: Option<(BeatTime, f64, i64)> = None;
    for per_bar in [1i64, 2, 4] {
        let step = bar.scale(1, per_bar);
        if step < min_slot {
            continue;
        }
        let grid = even_grid(start, end, step, tm);
        let hits = sal
            .structural
            .iter()
            .filter(|id| {
                m.notes
                    .iter()
                    .find(|n| n.id == **id)
                    .is_some_and(|n| grid.contains(&n.onset))
            })
            .count();
        let coverage = if sal.structural.is_empty() {
            0.0
        } else {
            hits as f64 / sal.structural.len() as f64
        };
        let penalty = (grid.len() as f64) / 64.0;
        let score = coverage - penalty;
        if best.is_none_or(|(_, b, _)| score > b) {
            best = Some((step, score, per_bar));
        }
    }
    match best {
        Some((step, score, per_bar)) => (
            step,
            format!(
                "{per_bar} slot(s) per bar explained the structural notes best, at {:.2}",
                score
            ),
        ),
        None => (
            bar,
            "no resolution qualified, so one chord per bar".to_string(),
        ),
    }
}

/// Removes boundaries that open a slot with nothing structural in it.
fn merge_empty(
    boundaries: Vec<BeatTime>,
    end: BeatTime,
    sal: &SalienceReport,
    m: &NoteSet,
    min_slot: BeatTime,
    max_slot: BeatTime,
) -> Vec<BeatTime> {
    if boundaries.len() < 2 {
        return boundaries;
    }
    let mut out: Vec<BeatTime> = vec![boundaries[0]];
    for i in 1..boundaries.len() {
        let a = boundaries[i];
        let b = boundaries.get(i + 1).copied().unwrap_or(end);
        let has_structural = sal.structural.iter().any(|id| {
            m.notes
                .iter()
                .find(|n| n.id == *id)
                .is_some_and(|n| n.onset >= a && n.onset < b)
        });
        // `out` is seeded above and only ever grows, so this fallback is
        // unreachable; it is written out rather than asserted so nothing here
        // can panic on caller input.
        let previous = out.last().copied().unwrap_or(a);
        let merged_len = b - previous;
        // Merging is only allowed while the result stays inside the profile's
        // ceiling; a four-bar chord is a decision, not an accident.
        if has_structural || merged_len > max_slot || (a - previous) >= min_slot * 4 {
            out.push(a);
        }
    }
    out
}

/// Splits a slot in which two structural notes of different pitch class both
/// carry real weight, provided the halves stay above the floor.
fn split_disagreements(
    boundaries: Vec<BeatTime>,
    end: BeatTime,
    sal: &SalienceReport,
    m: &NoteSet,
    min_slot: BeatTime,
) -> Vec<BeatTime> {
    let mut out = boundaries.clone();
    for i in 0..boundaries.len() {
        let a = boundaries[i];
        let b = boundaries.get(i + 1).copied().unwrap_or(end);
        if (b - a).scale(1, 2) < min_slot {
            continue;
        }
        let inside: Vec<&Note> = m
            .notes
            .iter()
            .filter(|n| n.onset >= a && n.onset < b && sal.is_structural(n.id))
            .collect();
        if inside.len() < 2 {
            continue;
        }
        let mid = a + (b - a).scale(1, 2);
        let disagree = inside.iter().any(|n| {
            n.onset < mid
                && inside
                    .iter()
                    .any(|q| q.onset >= mid && q.pitch_class() != n.pitch_class())
        });
        let second_half_strong = inside
            .iter()
            .any(|n| n.onset >= mid && sal.total(n.id) >= sal.threshold);
        if disagree && second_half_strong && !out.contains(&mid) {
            out.push(mid);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Turns boundaries into slots and fills in their contents.
fn materialise(
    boundaries: &[BeatTime],
    end: BeatTime,
    m: &NoteSet,
    sal: &SalienceReport,
    ph: &PhraseAnalysis,
) -> Vec<GridSlot> {
    let mut slots = Vec::new();
    for (i, a) in boundaries.iter().enumerate() {
        let b = boundaries.get(i + 1).copied().unwrap_or(end);
        if b <= *a {
            continue;
        }
        let melody_notes: Vec<NoteId> = m
            .notes
            .iter()
            .filter(|n| n.onset >= *a && n.onset < b)
            .map(|n| n.id)
            .collect();
        let structural_notes: Vec<NoteId> = melody_notes
            .iter()
            .copied()
            .filter(|id| sal.is_structural(*id))
            .collect();
        let is_cadential = ph.phrases.iter().any(|p| {
            p.notes
                .last()
                .is_some_and(|last| melody_notes.contains(last))
        });
        let weight = melody_notes
            .iter()
            .map(|id| sal.total(*id))
            .fold(0.0f64, f64::max);
        slots.push(GridSlot {
            start: *a,
            end: b,
            melody_notes,
            structural_notes,
            is_cadential,
            weight: crate::util::round6(weight),
        });
    }
    slots.sort_by(|x, y| x.start.cmp(&y.start).then(cmp_f64(y.weight, x.weight)));
    slots
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phrase::analyze_phrases;
    use crate::salience::{analyze_salience, SalienceWeights};
    use crate::testing::note_set;
    use theory_kb::KnowledgeBase;

    fn profile(id: &str) -> ResolvedProfile {
        KnowledgeBase::embedded()
            .resolve_profile(id)
            .expect("a profile")
    }

    fn grid_for(ns: &NoteSet, mode: GridMode, prof: &str) -> HarmonicGrid {
        let tm = ns.time_map.clone();
        let ph = analyze_phrases(ns, &tm);
        let sal = analyze_salience(ns, &ph, &tm, &SalienceWeights::default());
        build_grid(ns, &tm, &ph, &sal, mode, &profile(prof))
    }

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
        ])
    }

    #[test]
    fn mode_ids_and_parsing_round_trip() {
        assert_eq!(GridMode::parse("auto"), Some(GridMode::Auto));
        assert_eq!(GridMode::parse("existing"), Some(GridMode::Existing));
        assert_eq!(GridMode::parse("bars:2"), Some(GridMode::Bars(2.0)));
        assert_eq!(GridMode::parse("beats:0.5"), Some(GridMode::Beats(0.5)));
        assert_eq!(GridMode::parse("nope"), None);
        assert_eq!(GridMode::Bars(1.0).id(), "bars");
        assert_eq!(GridMode::default(), GridMode::Auto);
    }

    #[test]
    fn auto_does_not_put_a_chord_under_every_note() {
        let ns = eight_bar();
        let g = grid_for(&ns, GridMode::Auto, "common_practice");
        assert!(
            g.slots.len() < ns.notes.len(),
            "{} slots for {} notes",
            g.slots.len(),
            ns.notes.len()
        );
        assert!(!g.slots.is_empty());
    }

    #[test]
    fn a_short_weak_note_does_not_force_a_chord_change() {
        // A bar of C-major arpeggio with a decorative sixteenth on the "and"
        // of four. Nothing about that note may open a slot.
        let ns = note_set(&[
            ("C4", "0", "2"),
            ("E4", "2", "1"),
            ("G4", "3", "1/2"),
            ("F#4", "7/2", "1/4"),
            ("G4", "4", "4"),
        ]);
        let g = grid_for(&ns, GridMode::Auto, "common_practice");
        let weak = BeatTime::parse("7/2").expect("a rational");
        assert!(
            !g.has_boundary_at(weak),
            "a sixteenth on a weak beat opened a slot: {:?}",
            g.slots
                .iter()
                .map(|s| s.start.to_display())
                .collect::<Vec<_>>()
        );
        let slot = g.slot_at(weak).expect("the note is inside a slot");
        assert!(!slot.structural_notes.contains(&3));
    }

    #[test]
    fn bars_mode_produces_one_slot_per_bar() {
        let ns = eight_bar();
        let g = grid_for(&ns, GridMode::Bars(1.0), "common_practice");
        assert_eq!(g.mode_used, GridMode::Bars(1.0));
        for s in &g.slots {
            assert_eq!(s.length(), BeatTime::from_quarters(4));
        }
        assert!(g.rationale.contains("as requested"));
    }

    #[test]
    fn two_chords_per_bar_is_a_half_bar_grid() {
        let ns = eight_bar();
        let g = grid_for(&ns, GridMode::Bars(0.5), "common_practice");
        for s in &g.slots {
            assert_eq!(s.length(), BeatTime::from_quarters(2));
        }
    }

    #[test]
    fn beats_mode_uses_the_beat_unit() {
        let ns = eight_bar();
        let g = grid_for(&ns, GridMode::Beats(2.0), "common_practice");
        assert_eq!(g.mode_used, GridMode::Beats(2.0));
        for s in &g.slots {
            assert_eq!(s.length(), BeatTime::from_quarters(2));
        }
    }

    #[test]
    fn existing_mode_detects_a_rhythm_and_says_how() {
        let ns = eight_bar();
        let g = grid_for(&ns, GridMode::Existing, "common_practice");
        assert_eq!(g.mode_used, GridMode::Existing);
        assert!(g.rationale.contains("detected"));
        assert!(!g.slots.is_empty());
    }

    #[test]
    fn slots_are_contiguous_and_cover_the_melody() {
        let ns = eight_bar();
        let g = grid_for(&ns, GridMode::Auto, "common_practice");
        let (start, end) = ns.span();
        assert_eq!(g.slots.first().expect("a slot").start, start);
        assert_eq!(g.slots.last().expect("a slot").end, end);
        for w in g.slots.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
    }

    #[test]
    fn every_note_lands_in_exactly_one_slot() {
        let ns = eight_bar();
        let g = grid_for(&ns, GridMode::Auto, "common_practice");
        let mut ids: Vec<NoteId> = g
            .slots
            .iter()
            .flat_map(|s| s.melody_notes.clone())
            .collect();
        ids.sort_unstable();
        let mut expected: Vec<NoteId> = ns.notes.iter().map(|n| n.id).collect();
        expected.sort_unstable();
        assert_eq!(ids, expected);
    }

    #[test]
    fn the_profile_harmonic_rhythm_changes_the_grid() {
        let ns = eight_bar();
        let slow = grid_for(&ns, GridMode::Auto, "modal_ambient");
        let quick = grid_for(&ns, GridMode::Auto, "jazz_standard");
        assert!(
            slow.slots.len() <= quick.slots.len(),
            "modal ambient produced {} slots, jazz {}",
            slow.slots.len(),
            quick.slots.len()
        );
    }

    #[test]
    fn the_rationale_names_the_profile_in_auto_mode() {
        let ns = eight_bar();
        let g = grid_for(&ns, GridMode::Auto, "pop_rock");
        assert!(g.rationale.contains("pop_rock"), "{}", g.rationale);
        assert!(g.rationale.contains("phrase boundaries"));
    }

    #[test]
    fn a_cadential_slot_is_marked() {
        let ns = eight_bar();
        let g = grid_for(&ns, GridMode::Auto, "common_practice");
        assert!(g.slots.iter().any(|s| s.is_cadential));
    }

    #[test]
    fn structural_notes_are_a_subset_of_melody_notes() {
        let ns = eight_bar();
        let g = grid_for(&ns, GridMode::Auto, "common_practice");
        for s in &g.slots {
            for id in &s.structural_notes {
                assert!(s.melody_notes.contains(id));
            }
        }
    }

    #[test]
    fn the_grid_is_deterministic() {
        let ns = eight_bar();
        let a = grid_for(&ns, GridMode::Auto, "common_practice");
        let b = grid_for(&ns, GridMode::Auto, "common_practice");
        assert_eq!(
            a.to_json().to_canonical_string(),
            b.to_json().to_canonical_string()
        );
    }

    #[test]
    fn an_empty_melody_produces_an_empty_grid() {
        let ns = NoteSet::default();
        let tm = TimeMap::constant(120.0, TimeSignature::new(4, 4));
        let g = build_grid(
            &ns,
            &tm,
            &PhraseAnalysis::default(),
            &SalienceReport::default(),
            GridMode::Auto,
            &profile("common_practice"),
        );
        assert!(g.slots.is_empty());
        assert!(g.rationale.contains("empty"));
    }

    #[test]
    fn slot_json_reports_contents_and_weight() {
        let ns = eight_bar();
        let g = grid_for(&ns, GridMode::Auto, "common_practice");
        let j = g.to_json();
        assert_eq!(j.get("mode_used").and_then(Json::as_str), Some("auto"));
        let slots = j.get("slots").and_then(Json::as_arr).expect("slots");
        assert!(slots[0].get("weight").and_then(Json::as_f64).is_some());
        assert!(slots[0]
            .get("melody_notes")
            .and_then(Json::as_arr)
            .is_some());
    }
}
