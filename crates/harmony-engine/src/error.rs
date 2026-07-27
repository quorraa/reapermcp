//! The one error type this crate returns.

use std::fmt;

/// A generation failure, carrying one of the brief's stable error codes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HarmonyError {
    /// Stable machine-readable code, e.g. `"INVALID_ARGUMENT"`.
    pub code: String,
    /// Human-readable detail.
    pub message: String,
}

impl HarmonyError {
    /// Builds an error from a code and a message.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> HarmonyError {
        HarmonyError {
            code: code.into(),
            message: message.into(),
        }
    }

    /// A caller supplied a parameter outside its documented range.
    pub fn invalid_argument(message: impl Into<String>) -> HarmonyError {
        HarmonyError::new(INVALID_ARGUMENT, message)
    }

    /// The knowledge bundle does not contain something the engine needs.
    pub fn knowledge_missing(message: impl Into<String>) -> HarmonyError {
        HarmonyError::new(KNOWLEDGE_MISSING, message)
    }

    /// No candidate survived hard-constraint filtering.
    pub fn no_valid_candidate(message: impl Into<String>) -> HarmonyError {
        HarmonyError::new(NO_VALID_CANDIDATE, message)
    }

    /// The caller cancelled the request.
    pub fn cancelled() -> HarmonyError {
        HarmonyError::new(CANCELLED, "generation was cancelled by the caller")
    }

    /// The analysis does not carry enough material to harmonise.
    pub fn empty_analysis(message: impl Into<String>) -> HarmonyError {
        HarmonyError::new(EMPTY_ANALYSIS, message)
    }

    /// True when this is the cancellation error.
    pub fn is_cancelled(&self) -> bool {
        self.code == CANCELLED
    }
}

/// Code for a parameter outside its documented range.
pub const INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";
/// Code for a missing knowledge record.
pub const KNOWLEDGE_MISSING: &str = "KNOWLEDGE_MISSING";
/// Code for an empty candidate set after hard filtering.
pub const NO_VALID_CANDIDATE: &str = "NO_VALID_CANDIDATE";
/// Code for caller cancellation.
pub const CANCELLED: &str = "CANCELLED";
/// Code for an analysis with nothing to harmonise.
pub const EMPTY_ANALYSIS: &str = "EMPTY_ANALYSIS";

impl fmt::Display for HarmonyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for HarmonyError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(HarmonyError::invalid_argument("x").code, "INVALID_ARGUMENT");
        assert_eq!(
            HarmonyError::knowledge_missing("x").code,
            "KNOWLEDGE_MISSING"
        );
        assert_eq!(
            HarmonyError::no_valid_candidate("x").code,
            "NO_VALID_CANDIDATE"
        );
        assert_eq!(HarmonyError::empty_analysis("x").code, "EMPTY_ANALYSIS");
        assert!(HarmonyError::cancelled().is_cancelled());
    }

    #[test]
    fn display_includes_code_and_message() {
        let e = HarmonyError::new("A", "b");
        assert_eq!(e.to_string(), "A: b");
        let _: &dyn std::error::Error = &e;
    }
}
