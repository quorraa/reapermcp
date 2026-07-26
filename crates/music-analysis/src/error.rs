//! [`AnalysisError`] and its stable codes.
//!
//! Every code here is drawn from the product's global error-code list, so a
//! failure surfaced by the MCP server never has to be translated.

use music_domain::DomainError;
use qjson::{json_obj, Json};
use theory_kb::KbError;

/// A failure of the analysis pipeline.
///
/// Analysis is deliberately reluctant to fail: ambiguity is reported as a
/// warning with a reduced confidence, not as an error. An `AnalysisError` means
/// the *request* could not be honoured at all — an unknown profile, an
/// extraction mode whose required argument is missing, or material that does
/// not contain the line the caller asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysisError {
    /// Stable machine-readable code.
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
}

impl AnalysisError {
    /// Builds an error from a code and a message.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> AnalysisError {
        AnalysisError {
            code: code.into(),
            message: message.into(),
        }
    }

    /// The stable code.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// The explanation.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// A caller-supplied value was missing or structurally wrong.
    pub fn invalid_argument(message: impl Into<String>) -> AnalysisError {
        AnalysisError::new("INVALID_ARGUMENT", message)
    }

    /// Melody extraction could not produce the line the caller asked for.
    pub fn ambiguous_melody(message: impl Into<String>) -> AnalysisError {
        AnalysisError::new("AMBIGUOUS_MELODY", message)
    }

    /// The material carried no notes at all where notes were required.
    pub fn no_midi_source(message: impl Into<String>) -> AnalysisError {
        AnalysisError::new("NO_MIDI_SOURCE", message)
    }

    /// The knowledge base could not answer a lookup the pipeline depends on.
    pub fn knowledge_invalid(message: impl Into<String>) -> AnalysisError {
        AnalysisError::new("KNOWLEDGE_INVALID", message)
    }

    /// An invariant inside this crate was violated.
    pub fn internal(message: impl Into<String>) -> AnalysisError {
        AnalysisError::new("INTERNAL_ERROR", message)
    }

    /// A note violated its invariants.
    pub fn invalid_note(message: impl Into<String>) -> AnalysisError {
        AnalysisError::new("INVALID_NOTE", message)
    }

    /// JSON form.
    pub fn to_json(&self) -> Json {
        json_obj! {
            "code" => self.code.clone(),
            "message" => self.message.clone(),
        }
    }
}

/// Every code [`AnalysisError`] can carry, in declaration order.
pub const ERROR_CODES: &[&str] = &[
    "INVALID_ARGUMENT",
    "AMBIGUOUS_MELODY",
    "NO_MIDI_SOURCE",
    "KNOWLEDGE_INVALID",
    "INTERNAL_ERROR",
    "INVALID_NOTE",
];

impl std::fmt::Display for AnalysisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AnalysisError {}

impl From<DomainError> for AnalysisError {
    fn from(e: DomainError) -> AnalysisError {
        AnalysisError::new(e.code, e.message)
    }
}

impl From<KbError> for AnalysisError {
    fn from(e: KbError) -> AnalysisError {
        AnalysisError::new(
            "KNOWLEDGE_INVALID",
            format!("[{}] {}: {}", e.code, e.path, e.message),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_code_then_message() {
        let e = AnalysisError::invalid_argument("mode midi_channel needs a channel");
        assert_eq!(
            e.to_string(),
            "INVALID_ARGUMENT: mode midi_channel needs a channel"
        );
    }

    #[test]
    fn codes_are_unique_and_declared() {
        let mut seen = ERROR_CODES.to_vec();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len());
        for ctor in [
            AnalysisError::invalid_argument("x"),
            AnalysisError::ambiguous_melody("x"),
            AnalysisError::no_midi_source("x"),
            AnalysisError::knowledge_invalid("x"),
            AnalysisError::internal("x"),
            AnalysisError::invalid_note("x"),
        ] {
            assert!(ERROR_CODES.contains(&ctor.code()), "{}", ctor.code());
        }
    }

    #[test]
    fn json_form_carries_both_fields() {
        let e = AnalysisError::internal("boom");
        let j = e.to_json();
        assert_eq!(j.get("code").and_then(Json::as_str), Some("INTERNAL_ERROR"));
        assert_eq!(j.get("message").and_then(Json::as_str), Some("boom"));
    }

    #[test]
    fn domain_and_kb_errors_convert() {
        let d: AnalysisError = DomainError::invalid_note("bad").into();
        assert_eq!(d.code, "INVALID_NOTE");
        let k: AnalysisError = KbError::new("C", "P", "M").into();
        assert_eq!(k.code, "KNOWLEDGE_INVALID");
        assert!(k.message.contains("P"));
    }
}
