//! Chord detection from polyphonic material.
//!
//! Detection is scored, not matched. For every harmonic slot the sounding
//! pitch classes are collected with the time each one sounds, and every chord
//! quality in `knowledge/chord_qualities.json` is tried on every root. The
//! score rewards explaining what is there, penalises claiming what is not, and
//! adds context — the bass note, and whether the chord belongs to the key the
//! previous stage found.
//!
//! Two consequences worth naming. First, the chord identities come from the
//! knowledge bundle, so adding a quality to `knowledge/` makes it detectable
//! without touching this crate. Second, a detected chord carries its
//! `confidence` from the margin over the runner-up, so a slot that genuinely
//! could be two chords says so instead of picking one and looking certain.

use crate::grid::{GridSlot, HarmonicGrid};
use crate::key::KeyAnalysis;
use crate::util::{cmp_f64, round6};
use music_domain::prelude::*;
use theory_kb::KnowledgeBase;

/// Weight of a sounding pitch class the chord explains.
const EXPLAINED: f64 = 1.0;
/// Penalty per unit of sounding weight the chord cannot explain.
const UNEXPLAINED: f64 = 1.15;
/// Penalty **per chord tone** that is claimed but never sounds.
///
/// This is the parsimony term, and it is deliberately an absolute count rather
/// than a fraction. A fraction rewards richness — an eleventh chord missing
/// three of its six tones scores better per tone than a triad missing one —
/// so the detector would answer a modal vamp on D with `Dm11` because the
/// eleventh happens to cover a passing G. Counting the invented tones instead
/// makes the simplest chord that explains the material win.
const MISSING_TONE: f64 = 0.30;
/// Bonus when the lowest sounding pitch class is the chord's root.
const BASS_IS_ROOT: f64 = 0.35;
/// Bonus when every chord tone belongs to the detected key.
const DIATONIC: f64 = 0.25;
/// Bonus per sounding weight on the root, which anchors the identity.
const ROOT_WEIGHT: f64 = 0.30;
/// How much more a structural melody note counts than an ordinary one.
///
/// The harmony has to agree with the notes the melody leans on; it does not
/// have to contain every note that passes by. Without this the detector is
/// circular over a single line — it would pick whatever chord happens to
/// contain the passing tones, and every melody note would come back a chord
/// tone.
const STRUCTURAL_EMPHASIS: f64 = 3.0;
/// How much a melody note that is *not* structural counts.
const DECORATIVE_EMPHASIS: f64 = 0.2;
/// Floor on the metric weight a note's contribution is scaled by, so an
/// offbeat note still counts for something.
const MIN_METRIC_FACTOR: f64 = 0.25;
/// Penalty per chord tone beyond a triad.
///
/// Two chords may explain the same notes equally well; the smaller one is the
/// better reading, because it claims less.
const SIZE_PENALTY: f64 = 0.06;

/// Detects one chord per grid slot.
///
/// Slots with nothing sounding in them produce no chord, so the result is not
/// necessarily the same length as `grid.slots`.
pub fn detect_chords(
    notes: &NoteSet,
    grid: &HarmonicGrid,
    kb: &KnowledgeBase,
    key: &KeyAnalysis,
) -> Vec<ChordEvent> {
    let templates = templates(kb);
    if templates.is_empty() {
        return Vec::new();
    }
    let key_pcs = key.top_pcs(kb);
    let mut out = Vec::new();
    let mut next_id = 0u32;

    for slot in &grid.slots {
        let sounding = sounding_weights(notes, slot);
        let total: f64 = sounding.iter().sum();
        if total <= 0.0 {
            continue;
        }
        let bass = lowest_pc(notes, slot.start, slot.end);

        let mut scored: Vec<(f64, usize, ChordSpec)> = Vec::new();
        for (order, spec) in templates.iter().enumerate() {
            for root_pc in 0..12 {
                let Some(rooted) = transpose_to(spec, root_pc) else {
                    continue;
                };
                let s = score(&rooted, &sounding, total, bass, &key_pcs);
                scored.push((s, order, rooted));
            }
        }
        scored.sort_by(|a, b| {
            cmp_f64(b.0, a.0)
                .then(a.1.cmp(&b.1))
                .then(a.2.root_pc().cmp(&b.2.root_pc()))
        });
        let Some((best, _, spec)) = scored.first().cloned() else {
            continue;
        };
        let runner_up = scored.get(1).map(|(s, _, _)| *s).unwrap_or(0.0);
        let margin = (best - runner_up).max(0.0);
        let confidence = round6((0.45 + margin.min(0.5)).clamp(0.0, 0.99));

        let mut ev = ChordEvent::new(next_id, spec, slot.start, slot.end - slot.start);
        next_id += 1;
        ev.inference_source = "detected".to_string();
        ev.confidence = confidence;
        ev.inversion = inversion_of(&ev.spec, bass);
        if let Some(top) = key.top() {
            ev.local_tonic = Some((top.tonic, top.scale_id.clone()));
        }
        ev.original_symbol = Some(ev.spec.render_ascii());
        out.push(ev);
    }
    out
}

/// The chord vocabulary, as root-position specs on C, in catalogue order.
///
/// The first written symbol of each quality is the authoritative template: it
/// round-trips through the chord-symbol parser, so the detector and the parser
/// can never drift apart.
fn templates(kb: &KnowledgeBase) -> Vec<ChordSpec> {
    let mut out = Vec::new();
    for q in kb.chord_qualities() {
        let Some(sym) = q.symbol_examples.first() else {
            continue;
        };
        if let Ok(spec) = symbol::parse(sym) {
            if spec.bass.is_some() {
                continue;
            }
            out.push(spec);
        }
    }
    out
}

/// Transposes a C-rooted template onto `root_pc`, keeping the spelling sane.
fn transpose_to(spec: &ChordSpec, root_pc: i32) -> Option<ChordSpec> {
    let current = spec.root_pc();
    let delta = (root_pc - current).rem_euclid(12);
    if delta == 0 {
        return Some(spec.clone());
    }
    Some(spec.transpose(Interval::from_semitones_default(delta)))
}

/// Scores one chord against the sounding content of a slot.
fn score(
    spec: &ChordSpec,
    sounding: &[f64; 12],
    total: f64,
    bass: Option<i32>,
    key_pcs: &[i32],
) -> f64 {
    let pcs = spec.pitch_classes();
    if pcs.is_empty() {
        return f64::MIN;
    }
    let mut explained = 0.0;
    for pc in &pcs {
        explained += sounding[pc.rem_euclid(12) as usize];
    }
    let unexplained = total - explained;
    let missing = pcs
        .iter()
        .filter(|pc| sounding[pc.rem_euclid(12) as usize] <= 0.0)
        .count() as f64;

    let mut s = EXPLAINED * (explained / total)
        - UNEXPLAINED * (unexplained / total)
        - MISSING_TONE * (missing / pcs.len() as f64);
    s -= SIZE_PENALTY * (pcs.len() as f64 - 3.0).max(0.0);
    s += ROOT_WEIGHT * (sounding[spec.root_pc().rem_euclid(12) as usize] / total);
    if bass == Some(spec.root_pc().rem_euclid(12)) {
        s += BASS_IS_ROOT;
    }
    if !key_pcs.is_empty() && pcs.iter().all(|pc| key_pcs.contains(&pc.rem_euclid(12))) {
        s += DIATONIC;
    }
    s
}

/// Weight each pitch class carries inside a slot.
///
/// The base is sounding time, scaled twice. First by **where the note falls**:
/// harmony agrees with what lands on the beat, not with what slips past on the
/// last sixteenth. Second by **what the note is**: a structural melody note
/// counts triple, a decorative one barely counts, and anything the grid does
/// not call melody — an accompaniment, or the inner voices of a chord — counts
/// once.
fn sounding_weights(notes: &NoteSet, slot: &GridSlot) -> [f64; 12] {
    let mut out = [0.0f64; 12];
    for n in &notes.notes {
        if n.muted {
            continue;
        }
        let a = n.onset.max(slot.start);
        let b = n.end().min(slot.end);
        if b <= a {
            continue;
        }
        let emphasis = if slot.structural_notes.contains(&n.id) {
            STRUCTURAL_EMPHASIS
        } else if slot.melody_notes.contains(&n.id) {
            DECORATIVE_EMPHASIS
        } else {
            1.0
        };
        let metric = notes
            .time_map
            .metric_weight(n.onset)
            .clamp(MIN_METRIC_FACTOR, 1.0);
        out[n.pitch_class() as usize] += (b - a).as_f64() * emphasis * metric;
    }
    out
}

/// The lowest sounding pitch class in `[start, end)`.
fn lowest_pc(notes: &NoteSet, start: BeatTime, end: BeatTime) -> Option<i32> {
    notes
        .notes
        .iter()
        .filter(|n| !n.muted && n.onset < end && n.end() > start)
        .min_by_key(|n| (n.midi, n.id))
        .map(|n| n.pitch_class())
}

/// Which inversion the bass note implies.
fn inversion_of(spec: &ChordSpec, bass: Option<i32>) -> u8 {
    let Some(bass) = bass else {
        return 0;
    };
    let pcs = spec.pitch_classes();
    match pcs.iter().position(|pc| pc.rem_euclid(12) == bass) {
        Some(i) => i.min(u8::MAX as usize) as u8,
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::GridMode;
    use crate::testing::note_set;
    use theory_kb::KnowledgeBase;

    fn kb() -> &'static KnowledgeBase {
        KnowledgeBase::embedded()
    }

    fn one_slot(start: &str, end: &str) -> HarmonicGrid {
        HarmonicGrid {
            slots: vec![GridSlot {
                start: BeatTime::parse(start).expect("a rational"),
                end: BeatTime::parse(end).expect("a rational"),
                melody_notes: Vec::new(),
                structural_notes: Vec::new(),
                is_cadential: false,
                weight: 0.5,
            }],
            mode_used: GridMode::Bars(1.0),
            rationale: "test".to_string(),
        }
    }

    fn bars(n: i64) -> HarmonicGrid {
        let slots = (0..n)
            .map(|i| GridSlot {
                start: BeatTime::from_quarters(i * 4),
                end: BeatTime::from_quarters(i * 4 + 4),
                melody_notes: Vec::new(),
                structural_notes: Vec::new(),
                is_cadential: false,
                weight: 0.5,
            })
            .collect();
        HarmonicGrid {
            slots,
            mode_used: GridMode::Bars(1.0),
            rationale: "test".to_string(),
        }
    }

    #[test]
    fn a_c_major_triad_is_detected_as_c() {
        let ns = note_set(&[("C3", "0", "4"), ("E4", "0", "4"), ("G4", "0", "4")]);
        let chords = detect_chords(&ns, &one_slot("0", "4"), kb(), &KeyAnalysis::default());
        assert_eq!(chords.len(), 1);
        assert_eq!(chords[0].spec.render_ascii(), "C");
        assert_eq!(chords[0].inference_source, "detected");
    }

    #[test]
    fn a_dominant_seventh_is_not_reduced_to_a_triad() {
        let ns = note_set(&[
            ("G2", "0", "4"),
            ("B3", "0", "4"),
            ("D4", "0", "4"),
            ("F4", "0", "4"),
        ]);
        let chords = detect_chords(&ns, &one_slot("0", "4"), kb(), &KeyAnalysis::default());
        assert_eq!(chords[0].spec.render_ascii(), "G7");
        assert!(chords[0].spec.is_dominant_family());
    }

    #[test]
    fn a_minor_seventh_keeps_its_quality() {
        let ns = note_set(&[
            ("D3", "0", "4"),
            ("F3", "0", "4"),
            ("A3", "0", "4"),
            ("C4", "0", "4"),
        ]);
        let chords = detect_chords(&ns, &one_slot("0", "4"), kb(), &KeyAnalysis::default());
        assert_eq!(chords[0].spec.render_ascii(), "Dm7");
    }

    #[test]
    fn a_sus_chord_is_not_called_a_triad_with_a_wrong_third() {
        let ns = note_set(&[("C3", "0", "4"), ("F3", "0", "4"), ("G3", "0", "4")]);
        let chords = detect_chords(&ns, &one_slot("0", "4"), kb(), &KeyAnalysis::default());
        assert!(
            chords[0].spec.is_suspended(),
            "{}",
            chords[0].spec.render_ascii()
        );
    }

    #[test]
    fn the_bass_note_sets_the_inversion() {
        let ns = note_set(&[("E3", "0", "4"), ("G3", "0", "4"), ("C4", "0", "4")]);
        let chords = detect_chords(&ns, &one_slot("0", "4"), kb(), &KeyAnalysis::default());
        assert_eq!(chords[0].spec.root_pc(), 0);
        assert_eq!(chords[0].inversion, 1);
    }

    #[test]
    fn one_chord_is_produced_per_populated_slot() {
        let ns = note_set(&[
            ("C3", "0", "4"),
            ("E4", "0", "4"),
            ("G4", "0", "4"),
            ("F3", "8", "4"),
            ("A4", "8", "4"),
            ("C5", "8", "4"),
        ]);
        let chords = detect_chords(&ns, &bars(3), kb(), &KeyAnalysis::default());
        assert_eq!(
            chords.len(),
            2,
            "the empty middle bar must not invent a chord"
        );
        assert_eq!(chords[0].onset, BeatTime::ZERO);
        assert_eq!(chords[1].onset, BeatTime::from_quarters(8));
    }

    #[test]
    fn detected_chords_carry_a_confidence_and_a_symbol() {
        let ns = note_set(&[("C3", "0", "4"), ("E4", "0", "4"), ("G4", "0", "4")]);
        let chords = detect_chords(&ns, &one_slot("0", "4"), kb(), &KeyAnalysis::default());
        assert!(chords[0].confidence > 0.0 && chords[0].confidence <= 0.99);
        assert_eq!(
            chords[0].original_symbol.as_deref(),
            Some(chords[0].spec.render_ascii().as_str())
        );
    }

    #[test]
    fn the_key_context_is_recorded_on_each_chord() {
        let ns = note_set(&[("C3", "0", "4"), ("E4", "0", "4"), ("G4", "0", "4")]);
        let key = crate::key::analyze_key(kb(), &ns, &ns.time_map.clone(), None, None);
        let chords = detect_chords(&ns, &one_slot("0", "4"), kb(), &key);
        assert!(chords[0].local_tonic.is_some());
    }

    #[test]
    fn detection_is_deterministic() {
        let ns = note_set(&[("C3", "0", "4"), ("E4", "0", "4"), ("G4", "0", "4")]);
        let a = detect_chords(&ns, &one_slot("0", "4"), kb(), &KeyAnalysis::default());
        let b = detect_chords(&ns, &one_slot("0", "4"), kb(), &KeyAnalysis::default());
        assert_eq!(
            a[0].to_json().to_canonical_string(),
            b[0].to_json().to_canonical_string()
        );
    }

    #[test]
    fn a_single_note_still_yields_a_chord_hypothesis() {
        let ns = note_set(&[("C4", "0", "4")]);
        let chords = detect_chords(&ns, &one_slot("0", "4"), kb(), &KeyAnalysis::default());
        assert_eq!(chords.len(), 1);
        assert_eq!(chords[0].spec.root_pc(), 0);
    }

    #[test]
    fn an_empty_grid_detects_nothing() {
        let ns = note_set(&[("C4", "0", "4")]);
        let chords = detect_chords(&ns, &HarmonicGrid::default(), kb(), &KeyAnalysis::default());
        assert!(chords.is_empty());
    }
}
