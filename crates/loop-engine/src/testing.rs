//! Fixture-backed test support.
//!
//! Compiled into the library rather than hidden behind `#[cfg(test)]` so the
//! integration tests under `tests/` use exactly the same harness as the unit
//! tests, and so a downstream crate can reproduce a loop audit from a fixture
//! without re-implementing the wiring.

use crate::audit::{audit_detailed, LoopAudit, LoopInput};
use crate::boundary::LoopSpan;
use music_analysis::report::Analysis;
use music_domain::prelude::*;
use std::path::PathBuf;
use theory_kb::{KnowledgeBase, ResolvedProfile};

/// Every looping rule in `knowledge/rules/looping.json`, in file order.
///
/// The audit must consult all fourteen; a rule that is never evaluated is a
/// rule whose facts were never populated.
pub const LOOPING_RULE_IDS: &[&str] = &[
    "looping.no_hanging_note_past_loop_end",
    "looping.pickup_wraps_or_duplicates",
    "looping.exact_length_is_preserved",
    "looping.closed_tonic_prefers_dominant_to_tonic_wrap",
    "looping.modal_drone_does_not_require_dominant",
    "looping.open_dominant_ends_unresolved",
    "looping.seamless_color_uses_common_tones",
    "looping.bass_continuity_across_wrap",
    "looping.voice_leading_smooth_across_wrap",
    "looping.pedal_continues_across_wrap",
    "looping.harmonic_rhythm_stable_at_wrap",
    "looping.one_shot_ending_is_excluded_from_wrap_scoring",
    "looping.transition_ready_leaves_an_opening",
    "looping.layer_removal_at_wrap_preserves_harmony",
];

/// Every loop intent, in the order the brief lists them.
pub const ALL_INTENTS: &[LoopIntent] = &[
    LoopIntent::ClosedTonic,
    LoopIntent::OpenDominant,
    LoopIntent::ModalDrone,
    LoopIntent::SeamlessColor,
    LoopIntent::TransitionReady,
    LoopIntent::OneShotEnding,
];

/// Every style profile the product ships, in the order the brief lists them.
pub const PROFILE_IDS: &[&str] = &[
    "common_practice",
    "strict_counterpoint",
    "jazz_standard",
    "blues",
    "pop_rock",
    "neo_soul_rnb",
    "modal_ambient",
    "cinematic",
    "electronic_loop",
    "drum_and_bass",
];

/// The repository's shared `fixtures/` directory.
pub fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
}

/// Loads a shared fixture by its id, e.g. `"loops/dominant_wrap"`.
pub fn load_fixture(id: &str) -> Fixture {
    let path = fixture_root().join(format!("{id}.json"));
    Fixture::from_path(&path).unwrap_or_else(|e| panic!("fixture {id}: {e}"))
}

/// A loop audit's inputs, owned together so a [`LoopInput`] can borrow from one
/// place.
pub struct Harness {
    /// The embedded knowledge bundle.
    pub kb: &'static KnowledgeBase,
    /// The resolved style profile.
    pub profile: ResolvedProfile,
    /// The material.
    pub notes: NoteSet,
    /// The harmony.
    pub chords: Vec<ChordEvent>,
    /// The arrangement layers.
    pub parts: Vec<Part>,
    /// The tempo and meter map.
    pub time_map: TimeMap,
    /// The loop region and its intent.
    pub span: LoopSpan,
    /// A full analysis, when the test wants one.
    pub analysis: Option<Analysis>,
}

/// A 4/4 map at 120 bpm.
pub fn plain_time_map() -> TimeMap {
    TimeMap::constant(120.0, TimeSignature::new(4, 4))
}

/// Resolves a profile, panicking with a useful message when the id is wrong.
pub fn profile(id: &str) -> ResolvedProfile {
    KnowledgeBase::embedded()
        .resolve_profile(id)
        .unwrap_or_else(|e| panic!("profile {id}: {e}"))
}

impl Harness {
    /// An empty harness over a sixteen-quarter loop.
    pub fn empty(intent: LoopIntent, profile_id: &str) -> Harness {
        Harness {
            kb: KnowledgeBase::embedded(),
            profile: profile(profile_id),
            notes: NoteSet::sorted(Vec::new(), plain_time_map()),
            chords: Vec::new(),
            parts: Vec::new(),
            time_map: plain_time_map(),
            span: LoopSpan::new(BeatTime::ZERO, BeatTime::from_quarters(16), intent),
            analysis: None,
        }
    }

    /// A harness over a shared fixture.
    pub fn from_fixture(id: &str, intent: LoopIntent, profile_id: &str) -> Harness {
        let f = load_fixture(id);
        let span = match &f.loop_region {
            Some(l) => LoopSpan::new(l.start_qn, l.end_qn, intent),
            None => LoopSpan::new(BeatTime::ZERO, BeatTime::from_quarters(16), intent),
        };
        Harness {
            kb: KnowledgeBase::embedded(),
            profile: profile(profile_id),
            notes: f.note_set(),
            chords: f.chord_events(),
            parts: Vec::new(),
            time_map: f.time_map(),
            span,
            analysis: None,
        }
    }

    /// Replaces the loop intent.
    pub fn with_intent(mut self, intent: LoopIntent) -> Harness {
        self.span.intent = intent;
        self
    }

    /// Replaces the style profile.
    pub fn with_profile(mut self, profile_id: &str) -> Harness {
        self.profile = profile(profile_id);
        self
    }

    /// Replaces the span.
    pub fn with_span(mut self, start: BeatTime, end: BeatTime) -> Harness {
        self.span = LoopSpan::new(start, end, self.span.intent);
        self
    }

    /// Replaces the notes.
    pub fn with_notes(mut self, notes: Vec<Note>) -> Harness {
        self.notes = NoteSet::sorted(notes, self.time_map.clone());
        self
    }

    /// Replaces the chords, parsed from `(symbol, onset_qn, duration_qn)`.
    pub fn with_chords(mut self, spec: &[(&str, i64, i64)]) -> Harness {
        self.chords = chords(spec);
        self
    }

    /// Replaces the arrangement parts.
    pub fn with_parts(mut self, parts: Vec<Part>) -> Harness {
        self.parts = parts;
        self
    }

    /// The audit input.
    pub fn input(&self) -> LoopInput<'_> {
        LoopInput {
            span: self.span,
            notes: &self.notes,
            chords: &self.chords,
            parts: &self.parts,
            time_map: &self.time_map,
            analysis: self.analysis.as_ref(),
        }
    }

    /// Runs the audit, keeping the working.
    pub fn audit(&self) -> LoopAudit {
        audit_detailed(self.kb, &self.profile, &self.input())
    }

    /// Runs the audit and keeps only the frozen report.
    pub fn report(&self) -> LoopReport {
        self.audit().report
    }
}

/// Builds chord events from `(symbol, onset_qn, duration_qn)` triples.
pub fn chords(spec: &[(&str, i64, i64)]) -> Vec<ChordEvent> {
    spec.iter()
        .enumerate()
        .map(|(i, (sym, onset, dur))| {
            let parsed = music_domain::symbol::parse(sym)
                .unwrap_or_else(|e| panic!("chord symbol {sym:?}: {}", e.message));
            let mut e = ChordEvent::new(
                i as u32,
                parsed,
                BeatTime::from_quarters(*onset),
                BeatTime::from_quarters(*dur),
            );
            e.original_symbol = Some((*sym).to_string());
            e
        })
        .collect()
}

/// Builds a note from whole-quarter positions.
pub fn note(id: NoteId, midi: i32, onset: BeatTime, duration: BeatTime) -> Note {
    let mut n = Note::new(id, SpelledPitch::from_midi(midi, None), onset, duration);
    n.midi = midi;
    n
}

/// Builds a note at rational positions, e.g. `note_at(1, 60, (3, 2), (1, 2))`.
pub fn note_at(id: NoteId, midi: i32, onset: (i64, i64), duration: (i64, i64)) -> Note {
    note(
        id,
        midi,
        BeatTime::new(onset.0, onset.1),
        BeatTime::new(duration.0, duration.1),
    )
}

/// Builds an arrangement part.
pub fn part(name: &str, role: ArrangementRole, notes: Vec<Note>) -> Part {
    Part {
        role,
        name: name.to_string(),
        notes,
        instrument_profile: None,
        channel: 0,
        polyphonic: true,
    }
}

/// `fixtures/loops/dominant_wrap.json` — C, Am, F, G7 ending on the dominant.
pub fn dominant_wrap(intent: LoopIntent, profile_id: &str) -> Harness {
    Harness::from_fixture("loops/dominant_wrap", intent, profile_id)
}

/// `fixtures/loops/pickup_and_hanging_note.json` — a one-beat anacrusis and a
/// final note sustaining a quarter past the loop end.
pub fn pickup_and_hanging_note(intent: LoopIntent, profile_id: &str) -> Harness {
    Harness::from_fixture("loops/pickup_and_hanging_note", intent, profile_id)
}

/// `fixtures/progressions/modal_planing_loop.json` — planing major triads with
/// no functional dominant anywhere.
pub fn modal_planing_loop(intent: LoopIntent, profile_id: &str) -> Harness {
    Harness::from_fixture("progressions/modal_planing_loop", intent, profile_id)
}

/// A loop ending on V that restarts on ii, so the leading tone never resolves.
pub fn unresolved_dominant(intent: LoopIntent, profile_id: &str) -> Harness {
    Harness::empty(intent, profile_id).with_chords(&[
        ("Dm7", 0, 4),
        ("Am7", 4, 4),
        ("F", 8, 4),
        ("G7", 12, 4),
    ])
}

/// A loop whose bass jumps a major ninth across the wrap while the upper voices
/// still connect by step, so the bass is the only discontinuity.
pub fn bass_leap(intent: LoopIntent, profile_id: &str) -> Harness {
    Harness::empty(intent, profile_id).with_notes(vec![
        note(0, 36, BeatTime::ZERO, BeatTime::from_quarters(4)),
        note(1, 60, BeatTime::ZERO, BeatTime::from_quarters(4)),
        note(2, 64, BeatTime::ZERO, BeatTime::from_quarters(4)),
        note(
            3,
            50,
            BeatTime::from_quarters(12),
            BeatTime::from_quarters(4),
        ),
        note(
            4,
            59,
            BeatTime::from_quarters(12),
            BeatTime::from_quarters(4),
        ),
        note(
            5,
            65,
            BeatTime::from_quarters(12),
            BeatTime::from_quarters(4),
        ),
    ])
}

/// A loop whose bass holds but whose upper voices can only cross the wrap by
/// leaping an octave, so the voice leading is the only discontinuity.
pub fn voice_leading_leap(intent: LoopIntent, profile_id: &str) -> Harness {
    Harness::empty(intent, profile_id).with_notes(vec![
        note(0, 48, BeatTime::ZERO, BeatTime::from_quarters(4)),
        note(1, 52, BeatTime::ZERO, BeatTime::from_quarters(4)),
        note(2, 55, BeatTime::ZERO, BeatTime::from_quarters(4)),
        note(
            3,
            48,
            BeatTime::from_quarters(12),
            BeatTime::from_quarters(4),
        ),
        note(
            4,
            64,
            BeatTime::from_quarters(12),
            BeatTime::from_quarters(4),
        ),
        note(
            5,
            67,
            BeatTime::from_quarters(12),
            BeatTime::from_quarters(4),
        ),
    ])
}

/// A drone under a planing loop, carried across the boundary.
pub fn pedal_loop(intent: LoopIntent, profile_id: &str) -> Harness {
    let mut pedal = note(0, 38, BeatTime::ZERO, BeatTime::from_quarters(17));
    pedal.role = NoteRole::Pedal;
    pedal.articulation = Some(crate::boundary::CARRY_MARK.to_string());
    Harness::empty(intent, profile_id)
        .with_chords(&[("D", 0, 4), ("C", 4, 4), ("G", 8, 4), ("C", 12, 4)])
        .with_notes(vec![
            pedal,
            note(1, 62, BeatTime::ZERO, BeatTime::from_quarters(4)),
            note(
                2,
                60,
                BeatTime::from_quarters(12),
                BeatTime::from_quarters(4),
            ),
        ])
}

/// A drone that stops dead at the loop end and re-articulates on the repeat.
pub fn restarting_pedal(intent: LoopIntent, profile_id: &str) -> Harness {
    let mut pedal = note(0, 38, BeatTime::ZERO, BeatTime::from_quarters(16));
    pedal.role = NoteRole::Pedal;
    Harness::empty(intent, profile_id)
        .with_chords(&[("D", 0, 8), ("C", 8, 8)])
        .with_notes(vec![
            pedal,
            note(1, 62, BeatTime::ZERO, BeatTime::from_quarters(4)),
            note(
                2,
                60,
                BeatTime::from_quarters(12),
                BeatTime::from_quarters(4),
            ),
        ])
}

/// A drone that sounds past the loop end without being marked to carry.
///
/// `looping.pedal_continues_across_wrap` is bypassed by its own
/// `note_carry_policy_is_explicit` exception whenever the drone is marked, so
/// this is the shape in which that rule can actually apply — at the cost of the
/// hanging-note rule applying as well.
pub fn unmarked_pedal_overhang(intent: LoopIntent, profile_id: &str) -> Harness {
    let mut pedal = note(0, 38, BeatTime::ZERO, BeatTime::from_quarters(17));
    pedal.role = NoteRole::Pedal;
    Harness::empty(intent, profile_id)
        .with_chords(&[("D", 0, 4), ("C", 4, 4), ("G", 8, 4), ("C", 12, 4)])
        .with_notes(vec![
            pedal,
            note(1, 62, BeatTime::ZERO, BeatTime::from_quarters(4)),
            note(
                2,
                60,
                BeatTime::from_quarters(12),
                BeatTime::from_quarters(4),
            ),
        ])
}

/// A loop ending on a dominant whose bass jumps more than an octave, so the
/// bass rule's dominant exception is the thing being exercised.
pub fn dominant_bass_leap(intent: LoopIntent, profile_id: &str) -> Harness {
    Harness::empty(intent, profile_id)
        .with_chords(&[("C", 0, 4), ("Am", 4, 4), ("F", 8, 4), ("G7", 12, 4)])
        .with_notes(vec![
            note(0, 60, BeatTime::ZERO, BeatTime::from_quarters(4)),
            note(
                1,
                43,
                BeatTime::from_quarters(12),
                BeatTime::from_quarters(4),
            ),
        ])
}

/// A loop whose final-bar pad doubles a guide tone that also sounds at the loop
/// start, so removing the layer costs nothing.
pub fn layer_removal_covered(intent: LoopIntent, profile_id: &str) -> Harness {
    let pad = part(
        "pad",
        ArrangementRole::Pad,
        vec![note(
            10,
            71,
            BeatTime::from_quarters(12),
            BeatTime::from_quarters(4),
        )],
    );
    let keys = part(
        "keys",
        ArrangementRole::Comping,
        vec![
            note(30, 59, BeatTime::ZERO, BeatTime::from_quarters(4)),
            note(
                31,
                59,
                BeatTime::from_quarters(12),
                BeatTime::from_quarters(4),
            ),
        ],
    );
    let mut notes = pad.notes.clone();
    notes.extend(keys.notes.clone());
    Harness::empty(intent, profile_id)
        .with_chords(&[("C", 0, 4), ("Am", 4, 4), ("F", 8, 4), ("G7", 12, 4)])
        .with_notes(notes)
        .with_parts(vec![pad, keys])
}

/// A loop whose last slot is half the length of its first.
pub fn uneven_harmonic_rhythm(intent: LoopIntent, profile_id: &str) -> Harness {
    Harness::empty(intent, profile_id).with_chords(&[
        ("C", 0, 8),
        ("Am", 8, 4),
        ("F", 12, 2),
        ("G7", 14, 2),
    ])
}

/// A loop where one layer holds the final chord's third and then stops.
pub fn layer_removal(intent: LoopIntent, profile_id: &str) -> Harness {
    // G7 across the last bar: the pad is the only source of B, the third, and
    // it plays nothing at the loop start.
    let pad = part(
        "pad",
        ArrangementRole::Pad,
        vec![note(
            10,
            71,
            BeatTime::from_quarters(12),
            BeatTime::from_quarters(4),
        )],
    );
    let bass = part(
        "bass",
        ArrangementRole::Bass,
        vec![
            note(20, 48, BeatTime::ZERO, BeatTime::from_quarters(12)),
            note(
                21,
                43,
                BeatTime::from_quarters(12),
                BeatTime::from_quarters(4),
            ),
        ],
    );
    let mut notes = pad.notes.clone();
    notes.extend(bass.notes.clone());
    Harness::empty(intent, profile_id)
        .with_chords(&[("C", 0, 4), ("Am", 4, 4), ("F", 8, 4), ("G7", 12, 4)])
        .with_notes(notes)
        .with_parts(vec![pad, bass])
}

/// A four-on-the-floor percussion pattern that starts on the loop point.
pub fn aligned_percussion(intent: LoopIntent, profile_id: &str) -> Harness {
    let mut notes = Vec::new();
    for beat in 0..16i64 {
        let mut n = note(
            beat as NoteId,
            36,
            BeatTime::from_quarters(beat),
            BeatTime::new(1, 2),
        );
        n.role = NoteRole::Percussion;
        notes.push(n);
    }
    Harness::empty(intent, profile_id).with_notes(notes)
}

/// A percussion pattern whose first hit is late, so the loop is out of phase.
pub fn offset_percussion(intent: LoopIntent, profile_id: &str) -> Harness {
    let mut notes = Vec::new();
    for beat in 0..15i64 {
        let mut n = note(
            beat as NoteId,
            36,
            BeatTime::from_quarters(beat) + BeatTime::new(1, 2),
            BeatTime::new(1, 4),
        );
        n.role = NoteRole::Percussion;
        notes.push(n);
    }
    Harness::empty(intent, profile_id).with_notes(notes)
}

/// A span whose end is before its start.
pub fn inverted_span() -> Harness {
    let mut h = Harness::empty(LoopIntent::ClosedTonic, "common_practice");
    h.span = LoopSpan::new(
        BeatTime::from_quarters(8),
        BeatTime::from_quarters(4),
        LoopIntent::ClosedTonic,
    );
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_loop_fixtures_load() {
        for id in [
            "loops/dominant_wrap",
            "loops/pickup_and_hanging_note",
            "progressions/modal_planing_loop",
        ] {
            let f = load_fixture(id);
            assert!(f.loop_region.is_some(), "{id} declares no loop region");
            f.validate().unwrap_or_else(|e| panic!("{id}: {e}"));
        }
    }

    #[test]
    fn every_profile_id_resolves() {
        for id in PROFILE_IDS {
            let p = profile(id);
            assert_eq!(p.id, *id);
        }
    }

    #[test]
    fn the_looping_rule_list_matches_the_knowledge_base() {
        let kb = KnowledgeBase::embedded();
        let mut in_kb: Vec<&str> = kb
            .rules_in_domain(theory_kb::RuleDomain::Looping)
            .iter()
            .map(|r| r.id.as_str())
            .collect();
        in_kb.sort_unstable();
        let mut mine: Vec<&str> = LOOPING_RULE_IDS.to_vec();
        mine.sort_unstable();
        assert_eq!(in_kb, mine);
        assert_eq!(LOOPING_RULE_IDS.len(), 14);
    }
}
