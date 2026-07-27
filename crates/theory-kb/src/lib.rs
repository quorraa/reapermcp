//! `theory-kb` — the executable music-theory knowledge layer.
//!
//! This crate loads `knowledge/`, validates it, and **runs** it. The rule base
//! is not documentation with a lookup table bolted on: a [`rules::RuleEngine`]
//! evaluates 147 rules over a closed vocabulary of 116 predicates, in a style
//! profile that decides which rules are active and how much each one counts,
//! and reports every decision with the sources behind it.
//!
//! # Modules
//!
//! | Module | Purpose |
//! |---|---|
//! | [`model`] | one Rust type per knowledge record, plus the closed enums |
//! | [`load`] | [`KnowledgeBase`]: parsing, indexing, lookups, the content hash |
//! | [`validate`] | every rejection an external knowledge directory can earn |
//! | [`embedded`] | the generated `include_str!` table of the baked-in bundle |
//! | [`rules`] | [`rules::Tri`], [`rules::RuleContext`], [`rules::RuleEngine`] |
//! | [`profile`] | inheritance flattening, override merging, weight normalisation |
//! | [`query`] | the deterministic search behind `theory.search` |
//! | [`error`] | [`KbError`] |
//!
//! # Design commitments
//!
//! * **There is no single correct harmony.** The same sonority is a fault in
//!   one profile, a colour in another and the point in a third, so every
//!   result names the profile, the rules that fired, the exceptions that
//!   bypassed them and the sources behind them.
//! * **`Unknown` is not `false`.** A predicate the caller has not answered
//!   makes a rule `not_applicable`, never a guess.
//! * **Hard before soft.** Integrity and invariant rules are evaluated first
//!   and reject a candidate outright; everything else contributes to a score
//!   vector so unconventional-but-valid material stays reachable.
//! * **Determinism.** Nothing here iterates a `HashMap`; search ties break by
//!   id; the content hash is canonical-JSON SHA-256.
//!
//! # Example
//!
//! ```
//! use theory_kb::prelude::*;
//!
//! let kb = KnowledgeBase::embedded();
//! let profile = kb.resolve_profile("jazz_standard").expect("a profile");
//! let engine = RuleEngine::new(kb, &profile);
//!
//! let mut ctx = RuleContext::new();
//! ctx.set_event(RuleEvent::ChordPair);
//! ctx.set_str(facts::CHORD_FAMILY, "dominant");
//! ctx.set_str(facts::FUNCTION_CLASS, "dominant");
//! ctx.set_list(facts::SOUNDING_DEGREES, &["1", "3", "5", "b7"]);
//!
//! let outcome = engine.evaluate(&ctx);
//! assert!(outcome.is_acceptable());
//! ```

#![warn(missing_docs)]

pub mod embedded;
pub mod error;
pub mod load;
pub mod model;
pub mod profile;
pub mod query;
pub mod rules;
pub mod validate;

pub use error::KbError;
pub use load::KnowledgeBase;
pub use model::{
    ArrangementPattern, CadenceSchema, ChordQuality, ChordSymbolAlias, FunctionEntry,
    InstrumentProfile, IntervalRecord, Manifest, MelodyCompatibility, MidiRange, ModeRecord,
    ProgressionSchema, RelevantSection, RhythmPattern, RuleDomain, RuleEffect, RuleEvent, RuleKind,
    RuleOverride, RuleTrigger, ScaleRecord, SourceRecord, SourceRef, SpacingLimit, StyleProfile,
    TheoryRule, VoicingTemplate, PENDING_HASH,
};
pub use profile::ResolvedProfile;
pub use query::{SearchHit, SearchQuery};
pub use rules::{RuleContext, RuleEngine, RuleOutcome, Tri};

/// `music-domain` re-exported, so crates that depend only on `theory-kb` — such
/// as `xtask` — can reach the shared musical vocabulary and the fixture loader
/// without adding a manifest entry.
pub use music_domain;

/// The types most callers want in scope.
pub mod prelude {
    pub use crate::error::KbError;
    pub use crate::load::KnowledgeBase;
    pub use crate::model::{
        RuleDomain, RuleEvent, RuleKind, RuleOverride, StyleProfile, TheoryRule,
    };
    pub use crate::profile::ResolvedProfile;
    pub use crate::query::{SearchHit, SearchQuery};
    pub use crate::rules::{facts, RuleContext, RuleEngine, RuleOutcome, Tri};
    pub use music_domain::candidate::{RuleApplication, RuleStatus, Severity, SCORE_COMPONENTS};
}

#[cfg(test)]
mod contract_surface {
    //! Compile-time assertions that the frozen cross-crate API exists with the
    //! signatures the downstream engines are written against.

    use crate::prelude::*;
    use crate::validate::{validate, ValidateOptions};
    use qjson::Json;
    use std::collections::BTreeMap;
    use std::path::Path;

    #[test]
    fn knowledge_base_signatures() {
        let _: fn() -> &'static KnowledgeBase = KnowledgeBase::embedded;
        let _: fn(&Path) -> Result<KnowledgeBase, KbError> = KnowledgeBase::load_dir;
        let _: for<'a> fn(&'a KnowledgeBase) -> &'a str = KnowledgeBase::version;
        let _: for<'a> fn(&'a KnowledgeBase) -> &'a str = KnowledgeBase::schema_version;
        let _: for<'a> fn(&'a KnowledgeBase) -> &'a str = KnowledgeBase::content_hash;
        let _: for<'a> fn(&'a KnowledgeBase) -> &'a crate::Manifest = KnowledgeBase::manifest;
        let _: fn(&KnowledgeBase) -> BTreeMap<String, usize> = KnowledgeBase::counts;
        let _: for<'a> fn(&'a KnowledgeBase) -> &'a [crate::SourceRecord] = KnowledgeBase::sources;
        let _: for<'a> fn(&'a KnowledgeBase, &str) -> Option<&'a crate::SourceRecord> =
            KnowledgeBase::source;
        let _: for<'a> fn(&'a KnowledgeBase) -> &'a [TheoryRule] = KnowledgeBase::rules;
        let _: for<'a> fn(&'a KnowledgeBase, &str) -> Option<&'a TheoryRule> = KnowledgeBase::rule;
        let _: for<'a> fn(&'a KnowledgeBase, RuleDomain) -> Vec<&'a TheoryRule> =
            KnowledgeBase::rules_in_domain;
        let _: for<'a> fn(&'a KnowledgeBase, RuleEvent, &ResolvedProfile) -> Vec<&'a TheoryRule> =
            KnowledgeBase::rules_for_event;
        let _: for<'a> fn(&'a KnowledgeBase) -> &'a [StyleProfile] = KnowledgeBase::profiles;
        let _: for<'a> fn(&'a KnowledgeBase, &str) -> Option<&'a StyleProfile> =
            KnowledgeBase::profile;
        let _: fn(&KnowledgeBase, &str) -> Result<ResolvedProfile, KbError> =
            KnowledgeBase::resolve_profile;
        let _: for<'a> fn(&'a KnowledgeBase) -> &'a [crate::ScaleRecord] = KnowledgeBase::scales;
        let _: for<'a> fn(&'a KnowledgeBase, &str) -> Option<&'a crate::ScaleRecord> =
            KnowledgeBase::scale;
        let _: for<'a> fn(&'a KnowledgeBase) -> &'a [crate::ChordQuality] =
            KnowledgeBase::chord_qualities;
        let _: for<'a> fn(&'a KnowledgeBase, &str) -> Option<&'a crate::ChordQuality> =
            KnowledgeBase::chord_quality;
        let _: for<'a> fn(&'a KnowledgeBase) -> &'a [crate::VoicingTemplate] =
            KnowledgeBase::voicing_templates;
        let _: for<'a> fn(&'a KnowledgeBase) -> &'a [crate::ProgressionSchema] =
            KnowledgeBase::progressions;
        let _: for<'a> fn(&'a KnowledgeBase) -> &'a [crate::CadenceSchema] =
            KnowledgeBase::cadences;
        let _: for<'a> fn(&'a KnowledgeBase) -> &'a [crate::ArrangementPattern] =
            KnowledgeBase::arrangement_patterns;
        let _: for<'a> fn(&'a KnowledgeBase) -> &'a [crate::InstrumentProfile] =
            KnowledgeBase::instrument_profiles;
        let _: for<'a> fn(&'a KnowledgeBase, &str) -> Option<&'a crate::InstrumentProfile> =
            KnowledgeBase::instrument_profile;
        let _: for<'a> fn(&'a KnowledgeBase) -> &'a [crate::FunctionEntry] =
            KnowledgeBase::functions;
        let _: for<'a> fn(&'a KnowledgeBase) -> &'a [crate::ModeRecord] = KnowledgeBase::modes;
        let _: for<'a> fn(&'a KnowledgeBase) -> &'a [crate::IntervalRecord] =
            KnowledgeBase::intervals;
        let _: fn(&KnowledgeBase) -> Json = KnowledgeBase::catalog_json;
        let _: fn(&KnowledgeBase, &SearchQuery) -> Vec<SearchHit> = KnowledgeBase::search;
    }

    #[test]
    fn profile_and_engine_signatures() {
        let _: fn(&ResolvedProfile, &str) -> f64 = ResolvedProfile::weight;
        let _: fn(&ResolvedProfile, &str) -> f64 = ResolvedProfile::rule_multiplier;
        let _: fn(&ResolvedProfile, &str) -> bool = ResolvedProfile::is_rule_enabled;
        let _: fn(&ResolvedProfile, &str) -> Option<f64> = ResolvedProfile::field_f64;
        let _: for<'a> fn(&'a ResolvedProfile, &str) -> Option<&'a str> =
            ResolvedProfile::field_str;
        let _: fn(&ResolvedProfile, &str) -> Option<bool> = ResolvedProfile::field_bool;
        let _: fn(&ResolvedProfile, &TheoryRule) -> bool = ResolvedProfile::applies_to;

        let _: fn() -> RuleContext = RuleContext::new;
        let _: fn(&mut RuleContext, &str, bool) = RuleContext::set_bool;
        let _: fn(&mut RuleContext, &str, f64) = RuleContext::set_num;
        let _: fn(&mut RuleContext, &str, &str) = RuleContext::set_str;
        let _: fn(&RuleContext, &str) -> Option<bool> = RuleContext::get_bool;
        let _: fn(&RuleContext, &str) -> Option<f64> = RuleContext::get_num;
        let _: for<'a> fn(&'a RuleContext, &str) -> Option<&'a str> = RuleContext::get_str;
        let _: fn(&RuleContext) -> Option<RuleEvent> = RuleContext::event;
        let _: fn(&mut RuleContext, RuleEvent) = RuleContext::set_event;

        let _: fn(&'static KnowledgeBase, &'static ResolvedProfile) -> RuleEngine<'static> =
            RuleEngine::new;
        let _: fn() -> &'static [&'static str] = RuleEngine::known_predicates;

        // The two evaluation entry points carry a lifetime parameter, so they
        // are pinned by calling them rather than by coercing to a fn pointer.
        let kb = KnowledgeBase::embedded();
        let profile = kb.resolve_profile("common_practice").expect("a profile");
        let engine = RuleEngine::new(kb, &profile);
        let ctx = RuleContext::new();
        let outcome: RuleOutcome = engine.evaluate(&ctx);
        assert!(outcome.applications.is_empty(), "no event means no rules");
        let app: RuleApplication = engine.evaluate_rule(&kb.rules()[0], &ctx);
        assert_eq!(app.status, RuleStatus::NotApplicable);
    }

    #[test]
    fn error_and_validation_signatures() {
        let e = KbError::new("C", "P", "M");
        let _: &dyn std::error::Error = &e;
        assert_eq!(e.code, "C");
        let _: fn(&KnowledgeBase, &ValidateOptions) -> Result<(), Vec<KbError>> = validate;
        let _: RuleStatus = RuleStatus::Applied;
        let _: Severity = Severity::Moderate;
        assert_eq!(SCORE_COMPONENTS.len(), 13);
    }

    #[test]
    fn tri_is_three_valued() {
        assert_ne!(Tri::True, Tri::Unknown);
        assert_ne!(Tri::False, Tri::Unknown);
    }
}
