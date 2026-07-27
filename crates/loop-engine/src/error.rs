//! The one error type this crate returns.

use std::fmt;

/// A loop-audit failure, carrying one of the brief's stable error codes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopError {
    /// Stable machine-readable code, e.g. `"INVALID_ARGUMENT"`.
    pub code: String,
    /// Human-readable detail.
    pub message: String,
}

impl LoopError {
    /// Builds an error from a code and a message.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> LoopError {
        LoopError {
            code: code.into(),
            message: message.into(),
        }
    }

    /// A caller supplied a parameter outside its documented range.
    pub fn invalid_argument(message: impl Into<String>) -> LoopError {
        LoopError::new(INVALID_ARGUMENT, message)
    }

    /// The loop span is empty or inverted, so there is nothing to wrap.
    pub fn invalid_loop_span(message: impl Into<String>) -> LoopError {
        LoopError::new(INVALID_LOOP_SPAN, message)
    }

    /// The knowledge bundle does not contain something the audit needs.
    pub fn knowledge_missing(message: impl Into<String>) -> LoopError {
        LoopError::new(KNOWLEDGE_MISSING, message)
    }

    /// The caller cancelled the request.
    pub fn cancelled() -> LoopError {
        LoopError::new(CANCELLED, "the loop audit was cancelled by the caller")
    }

    /// True when this is the cancellation error.
    pub fn is_cancelled(&self) -> bool {
        self.code == CANCELLED
    }
}

/// Code for a parameter outside its documented range.
pub const INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";
/// Code for an empty or inverted loop span.
pub const INVALID_LOOP_SPAN: &str = "INVALID_LOOP_SPAN";
/// Code for a missing knowledge record.
pub const KNOWLEDGE_MISSING: &str = "KNOWLEDGE_MISSING";
/// Code for caller cancellation.
pub const CANCELLED: &str = "CANCELLED";

impl fmt::Display for LoopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for LoopError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(LoopError::invalid_argument("x").code, INVALID_ARGUMENT);
        assert_eq!(LoopError::invalid_loop_span("x").code, INVALID_LOOP_SPAN);
        assert_eq!(LoopError::knowledge_missing("x").code, KNOWLEDGE_MISSING);
        assert!(LoopError::cancelled().is_cancelled());
    }

    #[test]
    fn display_includes_the_code_and_the_message() {
        let e = LoopError::invalid_loop_span("end is before start");
        assert_eq!(e.to_string(), "INVALID_LOOP_SPAN: end is before start");
    }
}
