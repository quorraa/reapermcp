//! Fixture-backed test support.
//!
//! Compiled into the library rather than hidden behind `#[cfg(test)]` so the
//! integration tests under `tests/` use exactly the same harness as the unit
//! tests, and so a downstream crate can reproduce a plan without re-implementing
//! the wiring.
//!
//! Building a candidate means running the whole harmony pipeline, which is the
//! expensive part of an arrangement test. The harness memoises candidates by
//! `(fixture, profile, seed)` behind a mutex so a two-hundred-test suite pays
//! for each one once.

use crate::error::ArrangementError;
use crate::params::ArrangementParams;
use crate::plan::{arrange, ArrangementPlan};
use harmony_engine::prelude::*;
use music_analysis::report::{analyze, Analysis, AnalyzeParams};
use music_domain::prelude::*;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use theory_kb::KnowledgeBase;

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

/// The fixtures this crate is exercised against.
pub const FIXTURES: &[&str] = &[
    "melodies/eight_bar_c_major",
    "melodies/blues_head_c",
    "melodies/dorian_vamp_d",
    "melodies/waltz_suspensions",
];

/// A fixture, its analysis, and a candidate to arrange.
#[derive(Clone)]
pub struct Harness {
    /// The fixture id.
    pub id: String,
    /// The embedded knowledge bundle.
    pub kb: &'static KnowledgeBase,
    /// The stage 1–4 reading.
    pub analysis: Analysis,
    /// The candidate being arranged.
    pub candidate: Candidate,
}

impl Harness {
    /// Arranges the candidate under a request.
    pub fn arrange(&self, params: &ArrangementParams) -> Result<ArrangementPlan, ArrangementError> {
        arrange(
            self.kb,
            &self.analysis,
            &self.candidate,
            params,
            &CancelFlag::new(),
        )
    }

    /// The time map the fixture declares.
    pub fn time_map(&self) -> TimeMap {
        self.analysis.extraction.melody.time_map.clone()
    }

    /// The span the candidate's chords cover.
    pub fn span(&self) -> (BeatTime, BeatTime) {
        let chords = &self.candidate.chords;
        (
            chords[0].onset,
            chords
                .iter()
                .map(ChordEvent::end)
                .fold(chords[0].onset, BeatTime::max),
        )
    }
}

/// The memo table behind [`harness`].
#[allow(clippy::type_complexity)] // A named alias would not make the OnceLock any clearer.
fn cache() -> &'static Mutex<BTreeMap<(String, String, u64), (Analysis, Candidate)>> {
    static CACHE: OnceLock<Mutex<BTreeMap<(String, String, u64), (Analysis, Candidate)>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Builds — or reuses — a harness for a fixture under a style profile.
///
/// # Panics
///
/// Panics when the fixture, the profile or the harmony pipeline fails, because
/// that is a broken test corpus rather than a runtime condition.
pub fn harness(fixture_id: &str, profile_id: &str) -> Harness {
    harness_seeded(fixture_id, profile_id, 7)
}

/// [`harness`] with an explicit generation seed.
///
/// # Panics
///
/// Panics for the same reasons [`harness`] does.
pub fn harness_seeded(fixture_id: &str, profile_id: &str, seed: u64) -> Harness {
    let key = (fixture_id.to_string(), profile_id.to_string(), seed);
    let kb = KnowledgeBase::embedded();
    if let Some((analysis, candidate)) = cache().lock().expect("the memo lock").get(&key) {
        return Harness {
            id: fixture_id.to_string(),
            kb,
            analysis: analysis.clone(),
            candidate: candidate.clone(),
        };
    }

    let fixture = harmony_engine::testing::load_fixture(fixture_id);
    let notes = fixture.note_set();
    let mut ap = AnalyzeParams {
        profile_id: profile_id.to_string(),
        ..AnalyzeParams::default()
    };
    if let Some(hint) = &fixture.key_hint {
        ap.tonal_center = Some((
            format!("{}{}", hint.tonic.0.as_char(), hint.tonic.1.ascii()),
            hint.scale_id.clone(),
        ));
    }
    let analysis =
        analyze(kb, fixture_id, &notes, &ap).unwrap_or_else(|e| panic!("{fixture_id}: {e}"));
    let params = GenerateParams {
        candidate_count: 1,
        seed,
        ..GenerateParams::default().with_profile(profile_id)
    };
    let candidates =
        generate_candidates(kb, &analysis, &params, &CancelFlag::new(), &mut |_, _| {})
            .unwrap_or_else(|e| panic!("{fixture_id} under {profile_id}: {e}"));
    let candidate = candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("{fixture_id} produced no candidate"));

    cache()
        .lock()
        .expect("the memo lock")
        .insert(key, (analysis.clone(), candidate.clone()));
    Harness {
        id: fixture_id.to_string(),
        kb,
        analysis,
        candidate,
    }
}

/// A candidate built directly from chord symbols, for tests that need exact
/// harmony rather than whatever the search chose.
///
/// # Panics
///
/// Panics on an unparseable symbol, which would be a mistake in the test.
pub fn chords_from(symbols: &[&str], slot_qn: i64) -> Vec<ChordEvent> {
    symbols
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let spec = music_domain::symbol::parse(s)
                .unwrap_or_else(|e| panic!("{s} is not a chord symbol: {e}"));
            ChordEvent::new(
                i as u32,
                spec,
                BeatTime::from_quarters(i as i64 * slot_qn),
                BeatTime::from_quarters(slot_qn),
            )
        })
        .collect()
}

/// Replaces a harness's chords, keeping its analysis.
pub fn with_chords(mut h: Harness, chords: Vec<ChordEvent>) -> Harness {
    h.candidate.chords = chords;
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_harness_loads_and_memoises() {
        let a = harness("melodies/eight_bar_c_major", "pop_rock");
        let b = harness("melodies/eight_bar_c_major", "pop_rock");
        assert_eq!(a.candidate.id, b.candidate.id);
        assert!(!a.candidate.chords.is_empty());
        assert!(a.span().1 > a.span().0);
    }

    #[test]
    fn chord_helpers_build_a_progression() {
        let chords = chords_from(&["Cmaj7", "A7", "Dm7", "G7"], 4);
        assert_eq!(chords.len(), 4);
        assert_eq!(chords[3].onset, BeatTime::from_quarters(12));
        let h = with_chords(
            harness("melodies/eight_bar_c_major", "jazz_standard"),
            chords,
        );
        assert_eq!(h.candidate.chords.len(), 4);
    }

    #[test]
    fn every_fixture_and_profile_loads() {
        for fixture in FIXTURES {
            let h = harness(fixture, "pop_rock");
            assert!(!h.candidate.chords.is_empty(), "{fixture}");
        }
        assert_eq!(PROFILE_IDS.len(), 10);
    }
}
