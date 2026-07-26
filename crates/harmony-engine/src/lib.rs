//! `harmony-engine` — the crate that decides what the music is.
//!
//! Everything a candidate contains is decided here, by the engine, from data:
//! which chords are available, which progression through them is best, how each
//! chord is voiced, where the bass goes, which three answers to return, and why.
//! No part of that reasoning is delegated to a language model, and no part of it
//! is a chord table in this crate — the vocabulary comes from `knowledge/`.
//!
//! # The pipeline
//!
//! | Stage | Module | What it does |
//! |---|---|---|
//! | 5 | [`candidates`] | the per-slot chord pool, with hard violations filtered out |
//! | 6 | [`search`] | beam search over whole phrases, three strategy biases |
//! | 7 | [`voicing`] | template realisation connected across time by a DP |
//! | — | [`voiceleading`] | the profile-driven audit of the realisation |
//! | 8 | [`bass`], [`countermelody`] | the supporting lines |
//! | 13 | [`diversity`] | clustering and marginal-relevance selection |
//! | 11 | [`explain`] | the decision trace, built from the record |
//! | — | [`generate`] | the wiring, and the final hard-constraint enforcement |
//! | — | [`reharm`] | the same search under preservation constraints |
//!
//! Supporting modules: [`ctx`] (the shared frame), [`keyctx`] (the tonal frame
//! and chord construction), [`factbuild`] (turning decisions into rule facts),
//! [`params`] (the request), [`error`], and [`testing`] (the fixture harness).
//!
//! # Design commitments
//!
//! * **Hard before soft.** A chord that fails a `hard_integrity` or
//!   `mathematical_invariant` rule is removed from the pool before the path
//!   search runs. Nothing else is ever removed: an unconventional choice keeps
//!   its place and pays for itself in the score vector, because a filter cannot
//!   express taste and a score can.
//! * **The score is a vector.** All thirteen `SCORE_COMPONENTS` are kept raw on
//!   every option, path and candidate; the profile-weighted total is stored
//!   alongside them, never instead of them.
//! * **The search is global.** Choosing the best chord at each beat
//!   independently is forbidden by the brief and wrong in any case; the beam
//!   optimises the whole phrase, with transition costs covering voice leading,
//!   functional or modal coherence, bass quality and phrase direction.
//! * **Style is not decoration.** Every profile-sensitive decision reads the
//!   profile: parallel-perfect treatment, extension density, modal tolerance,
//!   functional strength, harmonic rhythm, bass mode, diversity appetite.
//!   `strict_counterpoint` and `pop_rock` reach different verdicts about the
//!   same parallel fifth, and both verdicts are reported.
//! * **Determinism.** The same analysis, knowledge version, profile, parameters
//!   and seed produce the same candidates, byte for byte
//!   ([`generate::candidate_fingerprint`]). Seeds break ties and nothing else.
//! * **Traces are generated, never invented.** Every rule id in a trace exists
//!   in the bundle and every source id resolves, because both are checked
//!   before they are written.
//!
//! # Example
//!
//! ```no_run
//! use harmony_engine::prelude::*;
//! use music_analysis::report::{analyze, AnalyzeParams};
//! use theory_kb::KnowledgeBase;
//!
//! let kb = KnowledgeBase::embedded();
//! let notes = harmony_engine::testing::load_fixture("melodies/eight_bar_c_major").note_set();
//! let analysis = analyze(kb, "snapshot-1", &notes, &AnalyzeParams::default())?;
//!
//! let params = GenerateParams::default().with_profile("jazz_standard");
//! let candidates = generate_candidates(
//!     kb,
//!     &analysis,
//!     &params,
//!     &CancelFlag::new(),
//!     &mut |_fraction, _label| {},
//! )?;
//!
//! for candidate in &candidates {
//!     println!("{}: {}", candidate.label, candidate.trace.explanation);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

#![warn(missing_docs)]

pub mod bass;
pub mod candidates;
pub mod countermelody;
pub mod ctx;
pub mod diversity;
pub mod error;
pub mod explain;
pub mod factbuild;
pub mod generate;
pub mod keyctx;
pub mod params;
pub mod reharm;
pub mod search;
pub mod testing;
pub mod voiceleading;
pub mod voicing;

pub use error::HarmonyError;
pub use params::{
    BassMotion, CancelFlag, CountermelodyParams, GenerateParams, SearchConfig, VoicingParams,
};

/// The types most callers want in scope.
pub mod prelude {
    pub use crate::bass::generate_bass;
    pub use crate::candidates::{slot_options, ChordOption};
    pub use crate::countermelody::generate_countermelody;
    pub use crate::ctx::EngineContext;
    pub use crate::diversity::diversify;
    pub use crate::error::HarmonyError;
    pub use crate::explain::{build_trace, TraceInputs};
    pub use crate::generate::generate_candidates;
    pub use crate::keyctx::KeyContext;
    pub use crate::params::{
        BassMotion, CancelFlag, CountermelodyParams, GenerateParams, SearchConfig, VoicingParams,
    };
    pub use crate::reharm::{reharmonize, ReharmParams};
    pub use crate::search::{search_paths, HarmonicPath};
    pub use crate::voiceleading::{audit_voice_leading, Parallel, VoiceLeadingReport};
    pub use crate::voicing::voice_progression;
}

#[cfg(test)]
mod contract_surface {
    //! Compile-time assertions that the frozen `harmony-engine` API exists with
    //! the signatures `CONTRACTS.md` declares.

    use crate::prelude::*;
    use music_analysis::grid::GridSlot;
    use music_analysis::report::Analysis;
    use music_domain::prelude::*;
    use theory_kb::{InstrumentProfile, KnowledgeBase, ResolvedProfile};

    /// The signature of [`slot_options`].
    type SlotOptionsFn = fn(
        &KnowledgeBase,
        &ResolvedProfile,
        &Analysis,
        &GridSlot,
        &GenerateParams,
    ) -> Vec<ChordOption>;
    /// The signature of [`search_paths`].
    type SearchPathsFn = fn(
        &KnowledgeBase,
        &ResolvedProfile,
        &Analysis,
        &[Vec<ChordOption>],
        &GenerateParams,
        &SearchConfig,
        &CancelFlag,
        &mut dyn FnMut(f64, &str),
    ) -> Result<Vec<HarmonicPath>, HarmonyError>;
    /// The signature of [`voice_progression`].
    type VoiceProgressionFn = fn(
        &KnowledgeBase,
        &ResolvedProfile,
        &[ChordEvent],
        Option<&NoteSet>,
        &VoicingParams,
    ) -> Result<Vec<Voicing>, HarmonyError>;
    /// The signature of [`audit_voice_leading`].
    type AuditFn =
        fn(&KnowledgeBase, &ResolvedProfile, &[Voicing], &[ChordEvent]) -> VoiceLeadingReport;
    /// The signature of [`generate_bass`].
    type GenerateBassFn = fn(
        &KnowledgeBase,
        &ResolvedProfile,
        &[ChordEvent],
        &TimeMap,
        BassMotion,
        Option<&InstrumentProfile>,
        u64,
    ) -> Result<Vec<Note>, HarmonyError>;
    /// The signature of [`generate_countermelody`].
    type GenerateCountermelodyFn = fn(
        &KnowledgeBase,
        &ResolvedProfile,
        &NoteSet,
        &[ChordEvent],
        &Analysis,
        &CountermelodyParams,
        u64,
    ) -> Result<Vec<Note>, HarmonyError>;
    /// The signature of [`reharmonize`].
    type ReharmonizeFn = fn(
        &KnowledgeBase,
        &Analysis,
        &[ChordEvent],
        &ReharmParams,
        &CancelFlag,
    ) -> Result<Vec<Candidate>, HarmonyError>;
    /// The signature of [`generate_candidates`].
    type GenerateCandidatesFn = fn(
        &KnowledgeBase,
        &Analysis,
        &GenerateParams,
        &CancelFlag,
        &mut dyn FnMut(f64, &str),
    ) -> Result<Vec<Candidate>, HarmonyError>;

    #[test]
    fn frozen_signatures_exist() {
        let _: SlotOptionsFn = slot_options;
        let _: SearchPathsFn = search_paths;
        let _: VoiceProgressionFn = voice_progression;
        let _: AuditFn = audit_voice_leading;
        let _: GenerateBassFn = generate_bass;
        let _: GenerateCountermelodyFn = generate_countermelody;
        let _: ReharmonizeFn = reharmonize;
        let _: GenerateCandidatesFn = generate_candidates;
        let _: fn(Vec<HarmonicPath>, usize, &ResolvedProfile, u64) -> Vec<HarmonicPath> = diversify;
        let _: fn(&GenerateParams) -> Result<(), HarmonyError> = GenerateParams::validate;
        let _: fn(&GenerateParams) -> qjson::Json = GenerateParams::canonical_json;
        let _: fn() -> SearchConfig = SearchConfig::default;
    }

    #[test]
    fn frozen_enums_round_trip() {
        for m in BassMotion::all() {
            assert_eq!(BassMotion::parse(m.id()), Some(*m));
        }
    }

    #[test]
    fn error_type_is_an_error() {
        let e = HarmonyError::new("CODE", "message");
        let _: &dyn std::error::Error = &e;
        assert_eq!(e.code, "CODE");
        assert_eq!(e.message, "message");
    }

    #[test]
    fn parallel_carries_the_frozen_fields() {
        let p = Parallel {
            interval: Interval::P5,
            from_index: 0,
            voices: (VoiceId(0), VoiceId(1)),
            hidden: false,
        };
        assert_eq!(p.interval.semitones(), 7);
        assert_eq!(p.from_index, 0);
        assert!(!p.hidden);
    }
}
