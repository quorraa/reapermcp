//! Structured errors for the MCP surface.
//!
//! Two error vocabularies meet here. The bridge speaks the §19 codes defined by
//! [`reaper_ipc::codes`]; the server adds its own codes for the things that go
//! wrong before a command is ever written — an unknown tool, arguments that
//! fail their schema, an expired candidate id. Both are carried by the same
//! [`ToolError`] type and reach the client as a `tools/call` result with
//! `isError: true`, never as a JSON-RPC transport error.

use qjson::{json_obj, Json};

/// Server-side error codes, complementing [`reaper_ipc::codes`].
pub mod codes {
    /// The named tool does not exist.
    pub const UNKNOWN_TOOL: &str = "UNKNOWN_TOOL";
    /// The arguments failed the tool's declared `inputSchema`.
    pub const INVALID_ARGUMENTS: &str = "INVALID_ARGUMENTS";
    /// An argument was well-typed but semantically unusable.
    pub const INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";
    /// A server-issued id is unknown to this process.
    pub const UNKNOWN_ID: &str = "UNKNOWN_ID";
    /// A server-issued id existed but its time-to-live has elapsed.
    pub const EXPIRED_ID: &str = "EXPIRED_ID";
    /// The requested resource URI is not served by this server.
    pub const UNKNOWN_RESOURCE: &str = "UNKNOWN_RESOURCE";
    /// The requested prompt is not served by this server.
    pub const UNKNOWN_PROMPT: &str = "UNKNOWN_PROMPT";
    /// The knowledge bundle failed validation.
    pub const KNOWLEDGE_INVALID: &str = "KNOWLEDGE_INVALID";
    /// The style profile id is not in the knowledge bundle.
    pub const UNKNOWN_PROFILE: &str = "UNKNOWN_PROFILE";
    /// Analysis failed.
    pub const ANALYSIS_FAILED: &str = "ANALYSIS_FAILED";
    /// Candidate generation failed.
    pub const GENERATION_FAILED: &str = "GENERATION_FAILED";
    /// Arrangement generation failed.
    pub const ARRANGEMENT_FAILED: &str = "ARRANGEMENT_FAILED";
    /// A loop audit could not be performed.
    pub const LOOP_AUDIT_FAILED: &str = "LOOP_AUDIT_FAILED";
    /// The caller cancelled the request.
    pub const CANCELLED: &str = "CANCELLED";
    /// The tool produced output that fails its own declared `outputSchema`.
    ///
    /// This is always a server bug. It is reported rather than hidden so the
    /// contract between the declared schema and the implementation stays
    /// enforced at runtime as well as in tests.
    pub const OUTPUT_SCHEMA_VIOLATION: &str = "OUTPUT_SCHEMA_VIOLATION";
    /// Something went wrong that has no more specific code.
    pub const INTERNAL_ERROR: &str = "INTERNAL_ERROR";
    /// The session store is full.
    pub const STORE_FULL: &str = "STORE_FULL";
    /// A staged transaction id is not one this server issued.
    pub const UNKNOWN_TRANSACTION: &str = "UNKNOWN_TRANSACTION";

    /// Every code this module defines, sorted, for documentation and tests.
    pub const SERVER_VOCABULARY: &[&str] = &[
        ANALYSIS_FAILED,
        ARRANGEMENT_FAILED,
        CANCELLED,
        EXPIRED_ID,
        GENERATION_FAILED,
        INTERNAL_ERROR,
        INVALID_ARGUMENT,
        INVALID_ARGUMENTS,
        KNOWLEDGE_INVALID,
        LOOP_AUDIT_FAILED,
        OUTPUT_SCHEMA_VIOLATION,
        STORE_FULL,
        UNKNOWN_ID,
        UNKNOWN_PROFILE,
        UNKNOWN_PROMPT,
        UNKNOWN_RESOURCE,
        UNKNOWN_TOOL,
        UNKNOWN_TRANSACTION,
    ];
}

/// A tool-level failure.
///
/// Deliberately not a JSON-RPC error: the MCP specification reserves transport
/// errors for protocol problems, and a chord that cannot be voiced is not a
/// protocol problem.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolError {
    /// A stable machine-readable code.
    pub code: String,
    /// A human-readable message. Never contains note content.
    pub message: String,
    /// Structured, advisory detail. Branch on `code`, never on `details`.
    pub details: Json,
    /// What the caller can do about it, when there is something useful to say.
    pub remedy: Option<String>,
}

impl ToolError {
    /// A tool error with no details.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> ToolError {
        ToolError {
            code: code.into(),
            message: message.into(),
            details: Json::Obj(Default::default()),
            remedy: None,
        }
    }

    /// A tool error carrying structured detail.
    pub fn with_details(
        code: impl Into<String>,
        message: impl Into<String>,
        details: Json,
    ) -> ToolError {
        ToolError {
            code: code.into(),
            message: message.into(),
            details,
            remedy: None,
        }
    }

    /// Attaches a suggested remedy.
    pub fn remedy(mut self, remedy: impl Into<String>) -> ToolError {
        self.remedy = Some(remedy.into());
        self
    }

    /// `INVALID_ARGUMENT` shorthand.
    pub fn invalid_argument(message: impl Into<String>) -> ToolError {
        ToolError::new(codes::INVALID_ARGUMENT, message)
    }

    /// `INTERNAL_ERROR` shorthand.
    pub fn internal(message: impl Into<String>) -> ToolError {
        ToolError::new(codes::INTERNAL_ERROR, message)
    }

    /// The payload a failing `tools/call` returns as its `structuredContent`.
    pub fn to_json(&self) -> Json {
        let mut m = qjson::JsonMap::new();
        m.insert("ok", Json::Bool(false));
        m.insert("error_code", Json::Str(self.code.clone()));
        m.insert("message", Json::Str(self.message.clone()));
        m.insert("details", self.details.clone());
        if let Some(r) = &self.remedy {
            m.insert("remedy", Json::Str(r.clone()));
        }
        Json::Obj(m)
    }

    /// True when this failure means the REAPER bridge is not running.
    pub fn is_bridge_offline(&self) -> bool {
        self.code == reaper_ipc::codes::BRIDGE_OFFLINE
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ToolError {}

impl From<reaper_ipc::IpcError> for ToolError {
    fn from(e: reaper_ipc::IpcError) -> ToolError {
        let remedy = match e.code.as_str() {
            reaper_ipc::codes::BRIDGE_OFFLINE => Some(
                "start REAPER and run the QLabs_Reaper_MCP_Bridge.lua action, then retry"
                    .to_string(),
            ),
            reaper_ipc::codes::STALE_SNAPSHOT
            | reaper_ipc::codes::MIDI_CHANGED
            | reaper_ipc::codes::TEMPO_MAP_CHANGED
            | reaper_ipc::codes::PROJECT_CHANGED => Some(
                "the source material changed since the snapshot was taken; call \
                 reaper.inspect_selection again and regenerate"
                    .to_string(),
            ),
            reaper_ipc::codes::NO_MIDI_SOURCE => {
                Some("select one MIDI item, or open a MIDI editor, then retry".to_string())
            }
            reaper_ipc::codes::MULTIPLE_MIDI_SOURCES => {
                Some("select exactly one MIDI item".to_string())
            }
            _ => None,
        };
        ToolError {
            code: e.code,
            message: e.message,
            details: e.details,
            remedy,
        }
    }
}

impl From<music_domain::DomainError> for ToolError {
    fn from(e: music_domain::DomainError) -> ToolError {
        ToolError::new(e.code, e.message)
    }
}

impl From<theory_kb::KbError> for ToolError {
    fn from(e: theory_kb::KbError) -> ToolError {
        ToolError::with_details(
            codes::KNOWLEDGE_INVALID,
            e.message.clone(),
            json_obj! { "path" => e.path.clone(), "code" => e.code.clone() },
        )
    }
}

impl From<music_analysis::AnalysisError> for ToolError {
    fn from(e: music_analysis::AnalysisError) -> ToolError {
        ToolError::new(map_engine_code(&e.code, codes::ANALYSIS_FAILED), e.message)
    }
}

impl From<harmony_engine::HarmonyError> for ToolError {
    fn from(e: harmony_engine::HarmonyError) -> ToolError {
        ToolError::new(map_engine_code(&e.code, codes::GENERATION_FAILED), e.message)
    }
}

impl From<arrangement_engine::ArrangementError> for ToolError {
    fn from(e: arrangement_engine::ArrangementError) -> ToolError {
        ToolError::new(
            map_engine_code(&e.code, codes::ARRANGEMENT_FAILED),
            e.message,
        )
    }
}

impl From<loop_engine::LoopError> for ToolError {
    fn from(e: loop_engine::LoopError) -> ToolError {
        ToolError::new(map_engine_code(&e.code, codes::LOOP_AUDIT_FAILED), e.message)
    }
}

/// Maps an engine's own error code onto the MCP vocabulary.
///
/// The engines use lowercase snake-case codes for their internal categories;
/// the two that matter to a caller — a bad argument and a cancellation — get a
/// stable MCP code, and everything else collapses onto the caller's `fallback`
/// so a new engine code can never leak an unknown string into the protocol.
fn map_engine_code(code: &str, fallback: &'static str) -> String {
    match code {
        "invalid_argument" | "INVALID_ARGUMENT" => codes::INVALID_ARGUMENT.to_string(),
        "cancelled" | "CANCELLED" => codes::CANCELLED.to_string(),
        _ => fallback.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vocabulary_is_sorted_and_unique() {
        let mut sorted = codes::SERVER_VOCABULARY.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, codes::SERVER_VOCABULARY);
    }

    #[test]
    fn tool_error_payload_shape() {
        let e = ToolError::new(codes::UNKNOWN_TOOL, "no such tool").remedy("call tools/list");
        let v = e.to_json();
        assert_eq!(v.get("ok"), Some(&Json::Bool(false)));
        assert_eq!(v.str_field("error_code").unwrap(), codes::UNKNOWN_TOOL);
        assert_eq!(v.str_field("remedy").unwrap(), "call tools/list");
    }

    #[test]
    fn ipc_offline_gets_a_remedy() {
        let e: ToolError =
            reaper_ipc::IpcError::new(reaper_ipc::codes::BRIDGE_OFFLINE, "no heartbeat").into();
        assert!(e.is_bridge_offline());
        assert!(e.remedy.is_some());
    }

    #[test]
    fn engine_codes_collapse_onto_the_fallback() {
        assert_eq!(map_engine_code("invalid_argument", "X"), "INVALID_ARGUMENT");
        assert_eq!(map_engine_code("cancelled", "X"), "CANCELLED");
        assert_eq!(map_engine_code("something_new", "X"), "X");
    }

    #[test]
    fn display_is_code_then_message() {
        let e = ToolError::invalid_argument("bad seed");
        assert_eq!(e.to_string(), "INVALID_ARGUMENT: bad seed");
    }
}
