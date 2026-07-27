//! `arrangement-engine` — who plays what, where, and when they keep quiet.

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
