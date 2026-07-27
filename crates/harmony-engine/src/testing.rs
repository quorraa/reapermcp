//! Fixture-backed test support.
//!
//! This module is compiled into the library rather than hidden behind
//! `#[cfg(test)]` so the integration tests under `tests/` can use exactly the
//! same harness as the unit tests, and so a downstream crate can reproduce a
//! candidate from a fixture without re-implementing the wiring.

use crate::ctx::EngineContext;
use crate::params::GenerateParams;
use music_analysis::report::{analyze, Analysis, AnalyzeParams};
use music_domain::prelude::*;
use std::path::PathBuf;
use theory_kb::{KnowledgeBase, ResolvedProfile};

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

/// The melody fixtures the engine is exercised against.
pub const MELODY_FIXTURES: &[&str] = &[
    "melodies/eight_bar_c_major",
    "melodies/blues_head_c",
    "melodies/dorian_vamp_d",
    "melodies/chromatic_descent",
    "melodies/waltz_suspensions",
];

/// The repository's shared `fixtures/` directory.
pub fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("fixtures")
}

/// Loads a fixture by its id, e.g. `"melodies/eight_bar_c_major"`.
///
/// Every fixture this crate uses lives in the shared corpus; the golden tests
/// discover that corpus by walking it, so it can grow without a crate-local
/// copy.
pub fn load_fixture(id: &str) -> Fixture {
    let path = fixture_root().join(format!("{id}.json"));
    Fixture::from_path(&path).unwrap_or_else(|e| panic!("fixture {id}: {e}"))
}

/// A loaded fixture, its analysis and the request parameters, owned together so
/// an [`EngineContext`] can borrow from one place.
pub struct Harness {
    /// The fixture id.
    pub id: String,
    /// The embedded knowledge bundle.
    pub kb: &'static KnowledgeBase,
    /// The resolved profile.
    pub profile: ResolvedProfile,
    /// The stage 1–4 reading.
    pub analysis: Analysis,
    /// The generation request.
    pub params: GenerateParams,
    /// The melody as loaded.
    pub notes: NoteSet,
}

impl Harness {
    /// Borrows a generation context.
    pub fn context(&self) -> EngineContext<'_> {
        EngineContext::new(self.kb, &self.profile, &self.analysis, &self.params)
            .expect("the fixture analysis carries a key reading")
    }

    /// A copy of the harness with different request parameters.
    pub fn with_params(mut self, params: GenerateParams) -> Harness {
        self.params = params;
        self
    }
}

/// Builds a harness for a fixture id under a style profile.
pub fn harness(fixture_id: &str, profile_id: &str) -> Harness {
    harness_with(
        fixture_id,
        GenerateParams::default().with_profile(profile_id),
    )
}

/// Builds a harness with explicit request parameters.
pub fn harness_with(fixture_id: &str, params: GenerateParams) -> Harness {
    let kb = KnowledgeBase::embedded();
    let fixture = load_fixture(fixture_id);
    let notes = fixture.note_set();
    let profile = kb
        .resolve_profile(&params.profile_id)
        .unwrap_or_else(|e| panic!("profile {}: {e}", params.profile_id));
    let mut ap = AnalyzeParams {
        profile_id: params.profile_id.clone(),
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
    Harness {
        id: fixture_id.to_string(),
        kb,
        profile,
        analysis,
        params,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_fixture_loads_and_analyses() {
        for id in MELODY_FIXTURES {
            let h = harness(id, "common_practice");
            assert!(!h.analysis.grid.slots.is_empty(), "{id} produced no grid");
            assert!(
                !h.analysis.key.candidates.is_empty(),
                "{id} produced no key"
            );
        }
    }

    #[test]
    fn every_profile_resolves() {
        let kb = KnowledgeBase::embedded();
        for id in PROFILE_IDS {
            assert!(
                kb.resolve_profile(id).is_ok(),
                "profile {id} does not resolve"
            );
        }
        assert_eq!(PROFILE_IDS.len(), 10);
    }
}
