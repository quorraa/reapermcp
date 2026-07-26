//! `music-analysis` — stages 1 to 4 of the harmonisation pipeline.
//!
//! This crate turns a bag of MIDI notes into an explained musical reading: what
//! the melody is, how it is phrased, which notes the harmony must agree with,
//! what key or mode the material is in, where the chords may change, and what
//! every dissonance is doing. Nothing downstream — chord generation, voicing,
//! arrangement, looping — can be better than this reading, so the design
//! commitments here are about honesty rather than cleverness:
//!
//! * **Assumptions are reported, never hidden.** [`selection::extract`] in
//!   `auto` mode will separate voices and take the top line when it has to, but
//!   it says so, drops its confidence and raises `AMBIGUOUS_MELODY`. It never
//!   presents a guess as a reading.
//! * **Key detection ranks; it does not label.** [`key::analyze_key`] scores
//!   every collection in `theory-kb` on every tonic against thirteen named
//!   evidence sources, reports the breakdown, and lets a modal or blues answer
//!   win when the evidence says so.
//! * **Non-chord tones are classified from motion and timing**, never from
//!   pitch membership, and a genuinely ambiguous tone keeps several readings.
//! * **The harmonic grid is chosen before the chords are.** A short note on a
//!   weak beat cannot force a chord change.
//! * **Salience is a transparent weighted sum** whose components are reported
//!   individually and add up to the total.
//! * **Determinism.** The same notes, parameters and knowledge version produce
//!   byte-identical [`report::Analysis::to_json`]. No `HashMap` iteration
//!   reaches output; every emitted float is rounded to six decimals.
//!
//! # Modules
//!
//! | Module | Stage | Purpose |
//! |---|---|---|
//! | [`selection`] | 1 | melody extraction and its assumptions |
//! | [`voices`] | 1 | deterministic voice separation |
//! | [`phrase`] | 2a | phrases, subphrases, motives, melody statistics |
//! | [`salience`] | 2b | the transparent structural-weight model |
//! | [`key`] | 3 | ranked key and mode candidates, local regions |
//! | [`grid`] | 4 | harmonic-rhythm selection |
//! | [`nct`] | — | non-chord-tone hypotheses |
//! | [`chord_detect`] | — | chord detection from polyphonic material |
//! | [`report`] | — | [`report::analyze`] and the cached [`report::Analysis`] |
//! | [`error`] | — | [`error::AnalysisError`] |
//!
//! # Example
//!
//! ```
//! use music_analysis::prelude::*;
//! use music_domain::prelude::*;
//! use theory_kb::KnowledgeBase;
//!
//! let notes = NoteSet::sorted(
//!     vec![
//!         Note::new(0, SpelledPitch::parse("C4").unwrap(), BeatTime::ZERO, BeatTime::ONE),
//!         Note::new(1, SpelledPitch::parse("E4").unwrap(), BeatTime::ONE, BeatTime::ONE),
//!         Note::new(
//!             2,
//!             SpelledPitch::parse("G4").unwrap(),
//!             BeatTime::from_quarters(2),
//!             BeatTime::from_quarters(2),
//!         ),
//!     ],
//!     TimeMap::constant(120.0, TimeSignature::new(4, 4)),
//! );
//!
//! let kb = KnowledgeBase::embedded();
//! let analysis = analyze(kb, "snapshot-1", &notes, &AnalyzeParams::default()).unwrap();
//!
//! // The reading is ranked, not asserted.
//! assert!(!analysis.key.candidates.is_empty());
//! // And it is reproducible.
//! let again = analyze(kb, "snapshot-1", &notes, &AnalyzeParams::default()).unwrap();
//! assert_eq!(
//!     analysis.to_json().to_canonical_string(),
//!     again.to_json().to_canonical_string()
//! );
//! ```

#![warn(missing_docs)]

pub mod chord_detect;
pub mod error;
pub mod grid;
pub mod key;
pub mod nct;
pub mod phrase;
pub mod report;
pub mod salience;
pub mod selection;
pub mod voices;

mod util;

#[cfg(test)]
mod testing;

pub use error::AnalysisError;

/// The types most callers want in scope.
pub mod prelude {
    pub use crate::chord_detect::detect_chords;
    pub use crate::error::AnalysisError;
    pub use crate::grid::{build_grid, GridMode, GridSlot, HarmonicGrid};
    pub use crate::key::{analyze_key, KeyAnalysis, KeyCandidate, KEY_EVIDENCE};
    pub use crate::nct::classify_ncts;
    pub use crate::phrase::{analyze_phrases, melody_profile, MelodyProfile, PhraseAnalysis};
    pub use crate::report::{analyze, Analysis, AnalyzeParams, Strictness};
    pub use crate::salience::{
        analyze_salience, SalienceDetail, SalienceReport, SalienceWeights, SALIENCE_COMPONENTS,
    };
    pub use crate::selection::{extract, Extraction, ExtractionMode, ExtractionRequest};
    pub use crate::voices::separate_voices;
}

#[cfg(test)]
mod contract_surface {
    //! Compile-time assertions that the frozen `music-analysis` API exists with
    //! the signatures the downstream engines are written against.

    use crate::prelude::*;
    use music_domain::prelude::*;
    use qjson::Json;
    use std::collections::BTreeMap;
    use theory_kb::{KnowledgeBase, ResolvedProfile};

    /// The signature of [`analyze_key`], named so the assertion below stays
    /// readable.
    type AnalyzeKeyFn = fn(
        &KnowledgeBase,
        &NoteSet,
        &TimeMap,
        Option<(&str, &str)>,
        Option<BeatTime>,
    ) -> KeyAnalysis;
    /// The signature of [`build_grid`].
    type BuildGridFn = fn(
        &NoteSet,
        &TimeMap,
        &PhraseAnalysis,
        &SalienceReport,
        GridMode,
        &ResolvedProfile,
    ) -> HarmonicGrid;
    /// The signature of [`classify_ncts`].
    type ClassifyNctsFn = fn(
        &NoteSet,
        &[ChordEvent],
        &TimeMap,
        &KnowledgeBase,
        &ResolvedProfile,
    ) -> BTreeMap<NoteId, Vec<NctHypothesis>>;
    /// The signature of [`analyze`].
    type AnalyzeFn =
        fn(&KnowledgeBase, &str, &NoteSet, &AnalyzeParams) -> Result<Analysis, AnalysisError>;

    #[test]
    fn stage_one_signatures() {
        let _: fn(ExtractionMode) -> &'static str = ExtractionMode::id;
        let _: fn(&str) -> Option<ExtractionMode> = ExtractionMode::parse;
        let _: fn(&NoteSet, &ExtractionRequest) -> Result<Extraction, AnalysisError> = extract;
        let _: fn(&NoteSet, usize) -> Vec<Vec<NoteId>> = separate_voices;
        let _: fn(&Extraction) -> Json = Extraction::to_json;
    }

    #[test]
    fn stage_two_signatures() {
        let _: fn(&NoteSet, &TimeMap) -> PhraseAnalysis = analyze_phrases;
        let _: fn(&NoteSet) -> MelodyProfile = melody_profile;
        let _: fn(&NoteSet, &PhraseAnalysis, &TimeMap, &SalienceWeights) -> SalienceReport =
            analyze_salience;
        let _: fn() -> SalienceWeights = SalienceWeights::default;
        assert_eq!(SALIENCE_COMPONENTS.len(), 9);
    }

    #[test]
    fn stage_three_and_four_signatures() {
        let _: AnalyzeKeyFn = analyze_key;
        let _: BuildGridFn = build_grid;
        let _: fn(&NoteSet, &HarmonicGrid, &KnowledgeBase, &KeyAnalysis) -> Vec<ChordEvent> =
            detect_chords;
        assert_eq!(KEY_EVIDENCE.len(), 13);
    }

    #[test]
    fn classification_and_report_signatures() {
        let _: ClassifyNctsFn = classify_ncts;
        let _: AnalyzeFn = analyze;
        let _: fn(&Analysis) -> Json = Analysis::to_json;
        let _: fn(&Analysis) -> Json = Analysis::summary_json;
        let _: fn(Strictness) -> &'static str = Strictness::id;
        let _: fn(&str) -> Option<Strictness> = Strictness::parse;
    }

    #[test]
    fn candidate_fields_are_where_the_contract_says() {
        let kb = KnowledgeBase::embedded();
        let notes =
            crate::testing::note_set(&[("C4", "0", "1"), ("E4", "1", "1"), ("G4", "2", "2")]);
        let k = analyze_key(kb, &notes, &notes.time_map.clone(), None, None);
        let c: &KeyCandidate = k.candidates.first().expect("a candidate");
        let _: (Letter, Accidental) = c.tonic;
        let _: &String = &c.scale_id;
        let _: f64 = c.score;
        let _: f64 = c.confidence;
        let _: &Vec<(&'static str, f64)> = &c.evidence;
        let _: bool = c.is_modal;
        let _: f64 = k.gap;
        let _: bool = k.ambiguous;
        let _: &Vec<KeyRegion> = &k.regions;
    }
}
