//! `arrangement-engine` — who plays what, where, and when they keep quiet.

#![warn(missing_docs)]

pub mod density;
pub mod energy;
pub mod error;
pub mod masking;
pub mod params;
pub mod patterns;
pub mod phrase;
pub mod roles;
pub mod sections;

pub use error::ArrangementError;
pub use params::ArrangementParams;
