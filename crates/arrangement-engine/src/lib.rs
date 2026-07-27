//! `arrangement-engine` — who plays what, where, and when they keep quiet.
//!
//! Pipeline stage 9, and all of brief §10.8. A candidate arrives carrying
//! chords and a melody; a plan leaves carrying parts, each written for a named
//! instrument profile, in a register that was allocated rather than assumed,
//! on a rhythm that came out of the pattern catalogue rather than out of this
//! crate.
//!
//! # Modules
//!
//! | Module | Purpose |
//! |---|---|
//! | [`roles`] | matching a role to a catalogued pattern and an instrument |
//! | [`patterns`] | realising that pattern into notes |
//! | [`density`] | measuring and controlling density, length and contrast |
//! | [`masking`] | counting collisions, and pushing registers apart |
//! | [`phrase`] | phrase-level planning: entrances, answers, fills, rests |
//! | [`sections`] | section-level planning: density, register, layers, contrast |
//! | [`energy`] | whole-loop planning: the arc, the peak, the silence, the seam |
//! | [`plan`] | [`arrange`], the wiring and the final hard-constraint enforcement |
//! | [`params`] | the request |
//! | [`error`] | [`ArrangementError`] |
//! | [`testing`] | the fixture harness |
//!
//! # Design commitments
//!
//! * **The data decides.** Every range, register, rhythm, grid, velocity
//!   accent, articulation tendency, doubling permission, polyphony ceiling and
//!   low-interval limit is read from `knowledge/arrangement_patterns.json` and
//!   `knowledge/instrument_profiles.json`. There is not one hardcoded range or
//!   onset list anywhere in this crate, and no universal spacing law: the bass
//!   profile, the string section and the mallet each declare their own.
//! * **Contrast is more than volume.** Register, rhythm, density, texture,
//!   timbre metadata, harmony, dynamics, articulation, silence and role
//!   substitution are all real levers, and [`density::Contrast`] measures which
//!   of them actually moved. A section that differs from its neighbour only in
//!   velocity is a defect the rule base names, and [`sections::enforce_contrast`]
//!   makes sure it cannot happen.
//! * **Masking is measured, not asserted.** [`masking_report`] counts
//!   simultaneous notes inside a perfect fourth of each other and reports where
//!   and when; [`arrange`] measures before and after register separation and
//!   keeps the separation only if the number went down.
//! * **Three planning levels, as the brief specifies.** Phrase, section and
//!   whole-loop, each in its own module, each feeding the realisation rather
//!   than decorating it.
//! * **Hard constraints are enforced on the output.** Instrument range, MIDI
//!   bounds, strictly positive durations, span containment, monophony where the
//!   pattern declares it, and the instrument's polyphony ceiling are checked on
//!   the material that will actually be written.
//! * **Determinism.** The same candidate, parameters and seed produce the same
//!   plan JSON, byte for byte ([`ArrangementPlan::fingerprint`]). Nothing here
//!   iterates a `HashMap`.
//!
//! # Example
//!
//! ```no_run
//! use arrangement_engine::{arrange, ArrangementParams};
//! use harmony_engine::CancelFlag;
//! use music_domain::prelude::ArrangementRole;
//! use theory_kb::KnowledgeBase;
//! # fn demo(analysis: &music_analysis::report::Analysis,
//! #         candidate: &music_domain::prelude::Candidate)
//! #     -> Result<(), Box<dyn std::error::Error>> {
//! let kb = KnowledgeBase::embedded();
//! let params = ArrangementParams::default()
//!     .with_profile("pop_rock")
//!     .with_roles(&[ArrangementRole::Lead, ArrangementRole::Bass, ArrangementRole::Pad])
//!     .with_density(0.6)
//!     .with_register_spread(0.8);
//!
//! let plan = arrange(kb, analysis, candidate, &params, &CancelFlag::new())?;
//! for (part, metrics) in plan.parts.iter().zip(plan.metrics.iter()) {
//!     println!("{}: {} notes, {:?}", part.role.id(), metrics.notes, metrics.register);
//! }
//! println!("masking {} -> {}", plan.masking_before.count(), plan.masking_after.count());
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]

pub mod density;
pub mod energy;
pub mod error;
pub mod masking;
pub mod params;
pub mod patterns;
pub mod phrase;
pub mod plan;
pub mod roles;
pub mod sections;
pub mod testing;

pub use error::ArrangementError;
pub use masking::{masking_report, MaskingReport};
pub use params::ArrangementParams;
pub use patterns::realize_pattern;
pub use plan::{arrange, ArrangementPlan, RoleAssignment};

/// The types most callers want in scope.
pub mod prelude {
    pub use crate::density::{contrast, measure, Contrast, PartMetrics};
    pub use crate::energy::{Ending, LoopPlan};
    pub use crate::error::ArrangementError;
    pub use crate::masking::{masking_report, MaskingReport};
    pub use crate::params::ArrangementParams;
    pub use crate::patterns::{realize_pattern, RealizeOptions};
    pub use crate::phrase::PhraseFrame;
    pub use crate::plan::{arrange, ArrangementPlan, RoleAssignment};
    pub use crate::roles::Priority;
    pub use crate::sections::SectionPlan;
}
