//! The one error type this crate returns.

use std::fmt;

/// Everything that can go wrong while planning an arrangement.
///
/// The shape is fixed by `CONTRACTS.md`: a stable machine-readable `code` and a
/// human-readable `message`. Codes are drawn from the brief's error list so the
/// MCP layer can map them onto protocol errors without a translation table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrangementError {
    /// Stable machine-readable code, e.g. `"INVALID_ARGUMENT"`.
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
}

impl ArrangementError {
    /// Builds an error from a code and a message.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> ArrangementError {
        ArrangementError {
            code: code.into(),
            message: message.into(),
        }
    }

    /// A request parameter is outside its documented domain.
    pub fn invalid_argument(message: impl Into<String>) -> ArrangementError {
        ArrangementError::new("INVALID_ARGUMENT", message)
    }

    /// The knowledge bundle does not carry a record the request needs.
    pub fn knowledge_missing(message: impl Into<String>) -> ArrangementError {
        ArrangementError::new("KNOWLEDGE_MISSING", message)
    }

    /// The candidate carries nothing an arrangement could be built from.
    pub fn no_harmony(message: impl Into<String>) -> ArrangementError {
        ArrangementError::new("NO_HARMONY", message)
    }

    /// No role could be realised without violating a hard constraint.
    pub fn no_valid_plan(message: impl Into<String>) -> ArrangementError {
        ArrangementError::new("NO_VALID_CANDIDATE", message)
    }

    /// The caller raised the cancel flag.
    pub fn cancelled() -> ArrangementError {
        ArrangementError::new("CANCELLED", "the request was cancelled")
    }
}

impl fmt::Display for ArrangementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ArrangementError {}

impl From<harmony_engine::HarmonyError> for ArrangementError {
    fn from(e: harmony_engine::HarmonyError) -> ArrangementError {
        ArrangementError::new(e.code, e.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(
            ArrangementError::invalid_argument("x").code,
            "INVALID_ARGUMENT"
        );
        assert_eq!(
            ArrangementError::knowledge_missing("x").code,
            "KNOWLEDGE_MISSING"
        );
        assert_eq!(ArrangementError::no_harmony("x").code, "NO_HARMONY");
        assert_eq!(
            ArrangementError::no_valid_plan("x").code,
            "NO_VALID_CANDIDATE"
        );
        assert_eq!(ArrangementError::cancelled().code, "CANCELLED");
    }

    #[test]
    fn display_includes_both_halves() {
        let e = ArrangementError::new("CODE", "message");
        assert_eq!(e.to_string(), "CODE: message");
        let _: &dyn std::error::Error = &e;
    }

    #[test]
    fn harmony_errors_convert() {
        let h = harmony_engine::HarmonyError::new("CANCELLED", "stopped");
        let a: ArrangementError = h.into();
        assert_eq!(a.code, "CANCELLED");
    }
}
