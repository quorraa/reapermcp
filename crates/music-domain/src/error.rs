//! Domain-wide error type.
//!
//! Every fallible operation in this crate reports a [`DomainError`], a flat
//! `{ code, message }` pair. Codes are stable strings — they cross the MCP and
//! IPC boundaries verbatim, so they are treated as part of the public contract.
//! The bridge-level codes listed in the product brief's IPC section are all
//! available as constructors so that no caller has to spell a code by hand.

use std::fmt;

/// A structured, transport-friendly error.
///
/// `code` is a stable SCREAMING_SNAKE_CASE identifier; `message` is a human
/// readable, non-localised sentence that may name concrete values.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DomainError {
    /// Stable machine-readable code, e.g. `"INVALID_CHORD_SYMBOL"`.
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
}

impl DomainError {
    /// Builds an error from an arbitrary code and message.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        DomainError {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Returns the error code.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Returns the error message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Serializes to `{"code": ..., "message": ...}`.
    pub fn to_json(&self) -> qjson::Json {
        qjson::json_obj! {
            "code" => self.code.clone(),
            "message" => self.message.clone(),
        }
    }

    /// Reads back the JSON form produced by [`DomainError::to_json`].
    pub fn from_json(v: &qjson::Json) -> Result<DomainError, DomainError> {
        Ok(DomainError::new(v.str_field("code")?, v.str_field("message")?))
    }
}

/// Declares a family of one-line constructors, one per stable error code.
macro_rules! error_ctors {
    ($( $(#[$m:meta])* $name:ident => $code:literal ),* $(,)?) => {
        impl DomainError {
            $(
                $(#[$m])*
                pub fn $name(message: impl Into<String>) -> DomainError {
                    DomainError::new($code, message)
                }
            )*
        }

        /// Every error code this crate can produce, in declaration order.
        pub const ERROR_CODES: &[&str] = &[ $($code),* ];
    };
}

error_ctors! {
    /// A caller-supplied value was structurally wrong.
    invalid_argument => "INVALID_ARGUMENT",
    /// A pitch string or MIDI number could not be interpreted.
    invalid_pitch => "INVALID_PITCH",
    /// A chord symbol failed to parse.
    invalid_chord_symbol => "INVALID_CHORD_SYMBOL",
    /// A musical time value was malformed (bad rational, zero denominator, ...).
    invalid_time => "INVALID_TIME",
    /// A note violated its invariants (duration, range, velocity, ...).
    invalid_note => "INVALID_NOTE",
    /// A fixture file did not match the frozen fixture format.
    invalid_fixture => "INVALID_FIXTURE",
    /// JSON was syntactically valid but semantically unusable here.
    invalid_json => "INVALID_JSON",
    /// A scale definition was inconsistent.
    invalid_scale => "INVALID_SCALE",
    /// The Lua bridge is not reachable.
    bridge_offline => "BRIDGE_OFFLINE",
    /// The bridge speaks a different version than this build.
    bridge_version_mismatch => "BRIDGE_VERSION_MISMATCH",
    /// A request timed out waiting for a result file.
    ipc_timeout => "IPC_TIMEOUT",
    /// The IPC envelope carried an unexpected protocol version.
    ipc_protocol_mismatch => "IPC_PROTOCOL_MISMATCH",
    /// The installation token did not match.
    invalid_instance_token => "INVALID_INSTANCE_TOKEN",
    /// The request's expiry timestamp had already passed.
    expired_request => "EXPIRED_REQUEST",
    /// The request or result exceeded the configured size limit.
    payload_too_large => "PAYLOAD_TOO_LARGE",
    /// REAPER reported no active project.
    no_active_project => "NO_ACTIVE_PROJECT",
    /// The selection contained no MIDI source.
    no_midi_source => "NO_MIDI_SOURCE",
    /// The selection contained more than one MIDI source.
    multiple_midi_sources => "MULTIPLE_MIDI_SOURCES",
    /// Melody extraction could not choose a line with enough confidence.
    ambiguous_melody => "AMBIGUOUS_MELODY",
    /// The referenced media item no longer exists.
    source_item_missing => "SOURCE_ITEM_MISSING",
    /// The referenced take no longer exists.
    source_take_missing => "SOURCE_TAKE_MISSING",
    /// The snapshot the plan was built against is no longer current.
    stale_snapshot => "STALE_SNAPSHOT",
    /// The project changed between analysis and commit.
    project_changed => "PROJECT_CHANGED",
    /// The MIDI content changed between analysis and commit.
    midi_changed => "MIDI_CHANGED",
    /// The tempo map changed between analysis and commit.
    tempo_map_changed => "TEMPO_MAP_CHANGED",
    /// An edit plan failed validation.
    invalid_edit_plan => "INVALID_EDIT_PLAN",
    /// The embedded knowledge base failed validation.
    knowledge_invalid => "KNOWLEDGE_INVALID",
    /// The detected REAPER version is not supported.
    unsupported_reaper_version => "UNSUPPORTED_REAPER_VERSION",
    /// The undo point at the top of the stack was not created by this server.
    undo_not_owned => "UNDO_NOT_OWNED",
    /// The bridge reported an unexpected internal failure.
    internal_bridge_error => "INTERNAL_BRIDGE_ERROR",
    /// An invariant inside this crate was violated.
    internal => "INTERNAL_ERROR",
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for DomainError {}

impl From<qjson::JsonError> for DomainError {
    fn from(e: qjson::JsonError) -> Self {
        DomainError::invalid_json(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_code_then_message() {
        let e = DomainError::invalid_pitch("no such letter 'H'");
        assert_eq!(e.to_string(), "INVALID_PITCH: no such letter 'H'");
    }

    #[test]
    fn codes_are_unique_and_non_empty() {
        let mut seen: Vec<&str> = ERROR_CODES.to_vec();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "duplicate error code");
        assert!(ERROR_CODES.iter().all(|c| !c.is_empty()));
        assert!(ERROR_CODES.contains(&"BRIDGE_OFFLINE"));
        assert!(ERROR_CODES.contains(&"TEMPO_MAP_CHANGED"));
    }

    #[test]
    fn json_round_trip() {
        let e = DomainError::stale_snapshot("snapshot 3 is gone");
        let back = DomainError::from_json(&e.to_json()).expect("round trip");
        assert_eq!(e, back);
    }

    #[test]
    fn json_error_converts() {
        let je = qjson::Json::parse("{").unwrap_err();
        let de: DomainError = je.into();
        assert_eq!(de.code, "INVALID_JSON");
    }

    #[test]
    fn error_is_std_error() {
        let e = DomainError::internal("boom");
        let dynamic: &dyn std::error::Error = &e;
        assert!(dynamic.to_string().contains("boom"));
    }
}
