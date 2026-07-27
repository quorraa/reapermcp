//! [`IpcError`] and the closed structured error-code vocabulary.
//!
//! Every failure the client can produce, and every failure the bridge reports,
//! is an [`IpcError`] carrying a `code` from [`codes`], a human-readable
//! `message`, and a `details` object that is passed through from the bridge
//! **unflattened**.
//!
//! # Branch on `code`, never on `details`
//!
//! `details` is advisory and its shape depends on the code. Code strings are
//! stable; `details` keys are not.

use qjson::{Json, JsonMap};

/// The structured error codes shared by both sides of the IPC.
///
/// The first group is the brief §19 list, reproduced verbatim. The second group
/// is the bridge's documented extensions. The third group is produced by this
/// crate only — the bridge declares the first two of them so both sides share
/// one vocabulary, but never emits them.
pub mod codes {
    // ---- brief §19 ------------------------------------------------------

    /// `heartbeat.json` is missing, unparseable, not `online`, or stale.
    ///
    /// Client-produced.
    pub const BRIDGE_OFFLINE: &str = "BRIDGE_OFFLINE";
    /// No `<id>.result.json` appeared before the client deadline.
    ///
    /// Client-produced.
    pub const IPC_TIMEOUT: &str = "IPC_TIMEOUT";
    /// `require_bridge_version` was sent and does not equal the bridge version.
    pub const BRIDGE_VERSION_MISMATCH: &str = "BRIDGE_VERSION_MISMATCH";
    /// `protocol_version` is absent or is not [`crate::limits::PROTOCOL_VERSION`].
    pub const IPC_PROTOCOL_MISMATCH: &str = "IPC_PROTOCOL_MISMATCH";
    /// `instance_token` is absent, empty, or does not match the installation.
    pub const INVALID_INSTANCE_TOKEN: &str = "INVALID_INSTANCE_TOKEN";
    /// `now > expires_at + CLOCK_SKEW_SECONDS`.
    pub const EXPIRED_REQUEST: &str = "EXPIRED_REQUEST";
    /// The command file is larger than `MAX_REQUEST_BYTES`.
    pub const PAYLOAD_TOO_LARGE: &str = "PAYLOAD_TOO_LARGE";
    /// A project command arrived with no active REAPER project.
    pub const NO_ACTIVE_PROJECT: &str = "NO_ACTIVE_PROJECT";
    /// No active MIDI editor take and no selected MIDI item.
    pub const NO_MIDI_SOURCE: &str = "NO_MIDI_SOURCE";
    /// Two or more selected items have valid MIDI takes.
    pub const MULTIPLE_MIDI_SOURCES: &str = "MULTIPLE_MIDI_SOURCES";
    /// The requested melody extraction cannot be resolved unambiguously.
    pub const AMBIGUOUS_MELODY: &str = "AMBIGUOUS_MELODY";
    /// A named item GUID is not in the project.
    pub const SOURCE_ITEM_MISSING: &str = "SOURCE_ITEM_MISSING";
    /// A named take GUID is not in the project, or a `midi_hash` constraint has
    /// no take to hash.
    pub const SOURCE_TAKE_MISSING: &str = "SOURCE_TAKE_MISSING";
    /// `snapshot_hash` / `base_snapshot_hash` / `item_bounds` no longer match.
    pub const STALE_SNAPSHOT: &str = "STALE_SNAPSHOT";
    /// `project_uuid` or `state_change_count` mismatch.
    pub const PROJECT_CHANGED: &str = "PROJECT_CHANGED";
    /// `midi_hash` mismatch.
    pub const MIDI_CHANGED: &str = "MIDI_CHANGED";
    /// `tempo_map_hash` mismatch.
    pub const TEMPO_MAP_CHANGED: &str = "TEMPO_MAP_CHANGED";
    /// A structural plan violation, or a mutation the plan cannot support.
    pub const INVALID_EDIT_PLAN: &str = "INVALID_EDIT_PLAN";
    /// `plan.knowledge_version` is missing or malformed.
    pub const KNOWLEDGE_INVALID: &str = "KNOWLEDGE_INVALID";
    /// `GetAppVersion()` is unparseable, or the major version is below 6.
    pub const UNSUPPORTED_REAPER_VERSION: &str = "UNSUPPORTED_REAPER_VERSION";
    /// The top undo entry is not this MCP's last owned transaction.
    pub const UNDO_NOT_OWNED: &str = "UNDO_NOT_OWNED";
    /// Any Lua error caught by the bridge's guards.
    pub const INTERNAL_BRIDGE_ERROR: &str = "INTERNAL_BRIDGE_ERROR";

    // ---- bridge extensions ----------------------------------------------

    /// The body is not JSON or not an object; a bad `request_id`; an id that
    /// does not equal the filename stem; an unparseable timestamp; a `payload`
    /// that is not an object; an unknown enum value; a missing required field.
    pub const MALFORMED_REQUEST: &str = "MALFORMED_REQUEST";
    /// `command` is not on the allowlist. `details.allowed` lists the allowlist.
    pub const UNKNOWN_COMMAND: &str = "UNKNOWN_COMMAND";
    /// `request_id` was already processed by this bridge instance.
    pub const DUPLICATE_REQUEST: &str = "DUPLICATE_REQUEST";
    /// The encoded result would exceed `MAX_RESULT_BYTES`.
    pub const RESULT_TOO_LARGE: &str = "RESULT_TOO_LARGE";
    /// `commit_candidate` / `discard_candidate` found no tagged object.
    pub const TRANSACTION_NOT_FOUND: &str = "TRANSACTION_NOT_FOUND";

    // ---- client extensions ----------------------------------------------

    /// The operation was cancelled through a [`crate::CancelFlag`].
    ///
    /// Client-produced. Not part of the bridge vocabulary: the bridge cannot
    /// observe a client-side cancellation.
    pub const IPC_CANCELLED: &str = "IPC_CANCELLED";

    /// A hash the bridge sent does not reproduce from the data it sent with it.
    ///
    /// Client-produced. Raised by [`crate::Snapshot::verify`] when independently
    /// recomputing a canonical string yields a different digest, which means the
    /// two sides disagree about the hash contract or the payload was mangled in
    /// transit. This is an error, never a warning: a snapshot whose own hashes
    /// do not reproduce cannot be used as a staleness token.
    pub const SNAPSHOT_HASH_MISMATCH: &str = "SNAPSHOT_HASH_MISMATCH";

    /// A local filesystem operation on the IPC directory failed.
    ///
    /// Client-produced.
    pub const IPC_IO_ERROR: &str = "IPC_IO_ERROR";

    /// Every code in the brief §19 list plus the bridge's documented extensions,
    /// sorted. This is exactly the set recorded in
    /// `fixtures/mock-reaper/bridge/error-codes.json`.
    pub const BRIDGE_VOCABULARY: &[&str] = &[
        AMBIGUOUS_MELODY,
        BRIDGE_OFFLINE,
        BRIDGE_VERSION_MISMATCH,
        DUPLICATE_REQUEST,
        EXPIRED_REQUEST,
        INTERNAL_BRIDGE_ERROR,
        INVALID_EDIT_PLAN,
        INVALID_INSTANCE_TOKEN,
        IPC_PROTOCOL_MISMATCH,
        IPC_TIMEOUT,
        KNOWLEDGE_INVALID,
        MALFORMED_REQUEST,
        MIDI_CHANGED,
        MULTIPLE_MIDI_SOURCES,
        NO_ACTIVE_PROJECT,
        NO_MIDI_SOURCE,
        PAYLOAD_TOO_LARGE,
        PROJECT_CHANGED,
        RESULT_TOO_LARGE,
        SOURCE_ITEM_MISSING,
        SOURCE_TAKE_MISSING,
        STALE_SNAPSHOT,
        TEMPO_MAP_CHANGED,
        TRANSACTION_NOT_FOUND,
        UNDO_NOT_OWNED,
        UNKNOWN_COMMAND,
        UNSUPPORTED_REAPER_VERSION,
    ];

    /// Codes this crate produces that the bridge never emits, sorted.
    pub const CLIENT_ONLY: &[&str] = &[
        BRIDGE_OFFLINE,
        IPC_CANCELLED,
        IPC_IO_ERROR,
        IPC_TIMEOUT,
        SNAPSHOT_HASH_MISMATCH,
    ];

    /// True when `code` is part of the vocabulary both sides share.
    pub fn is_known(code: &str) -> bool {
        BRIDGE_VOCABULARY.contains(&code) || CLIENT_ONLY.contains(&code)
    }
}

/// A structured IPC failure.
///
/// `code` is one of [`codes`]; `message` is human-readable and capped at
/// [`crate::limits::MAX_ERROR_MESSAGE_BYTES`]; `details` is an object whose
/// shape depends on `code` and which is preserved byte for byte when the error
/// came from the bridge.
#[derive(Clone, Debug, PartialEq)]
pub struct IpcError {
    /// The stable error code. Branch on this.
    pub code: String,
    /// Human-readable explanation. Never branch on this.
    pub message: String,
    /// Advisory structured detail. Always a `Json::Obj`. Never branch on this.
    pub details: Json,
}

impl IpcError {
    /// Builds an error with an empty `details` object.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> IpcError {
        IpcError {
            code: code.into(),
            message: truncate(message.into()),
            details: Json::Obj(JsonMap::new()),
        }
    }

    /// Builds an error carrying structured detail.
    ///
    /// A `details` value that is not an object is stored under the key `value`
    /// so the invariant "`details` is an object" always holds.
    pub fn with_details(
        code: impl Into<String>,
        message: impl Into<String>,
        details: Json,
    ) -> IpcError {
        let details = match details {
            Json::Obj(m) => Json::Obj(m),
            other => {
                let mut m = JsonMap::new();
                m.insert("value", other);
                Json::Obj(m)
            }
        };
        IpcError {
            code: code.into(),
            message: truncate(message.into()),
            details,
        }
    }

    /// Parses a wire error object (`{code, message, details}`).
    ///
    /// The bridge's `code` and `details` are preserved exactly, including codes
    /// this build does not recognise — an unknown code is a bridge that is newer
    /// than this client, not a reason to discard the information.
    pub fn from_wire(value: &Json) -> IpcError {
        let obj = match value.as_obj() {
            Some(o) => o,
            None => {
                return IpcError::with_details(
                    codes::INTERNAL_BRIDGE_ERROR,
                    "bridge error object was not a JSON object",
                    value.clone(),
                )
            }
        };
        let code = obj
            .get("code")
            .and_then(Json::as_str)
            .unwrap_or(codes::INTERNAL_BRIDGE_ERROR)
            .to_string();
        let message = obj
            .get("message")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let details = match obj.get("details") {
            Some(Json::Obj(m)) => Json::Obj(m.clone()),
            Some(Json::Null) | None => Json::Obj(JsonMap::new()),
            Some(other) => {
                let mut m = JsonMap::new();
                m.insert("value", other.clone());
                Json::Obj(m)
            }
        };
        IpcError {
            code,
            message: truncate(message),
            details,
        }
    }

    /// The wire form of this error: `{code, message, details}`.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("code", Json::Str(self.code.clone()));
        m.insert("message", Json::Str(self.message.clone()));
        m.insert("details", self.details.clone());
        Json::Obj(m)
    }

    /// Reads a string out of `details`, for callers that want to log detail
    /// without branching on it.
    pub fn detail_str(&self, key: &str) -> Option<&str> {
        self.details.get(key).and_then(Json::as_str)
    }

    /// True when the bridge could not be reached at all.
    pub fn is_bridge_offline(&self) -> bool {
        self.code == codes::BRIDGE_OFFLINE
    }

    /// True when the client deadline elapsed with no result.
    pub fn is_timeout(&self) -> bool {
        self.code == codes::IPC_TIMEOUT
    }

    /// True when the project moved underneath a snapshot or plan.
    ///
    /// These are the codes that mean "re-inspect and re-generate", as opposed
    /// to "fix the request".
    pub fn is_stale(&self) -> bool {
        matches!(
            self.code.as_str(),
            codes::STALE_SNAPSHOT
                | codes::PROJECT_CHANGED
                | codes::MIDI_CHANGED
                | codes::TEMPO_MAP_CHANGED
        )
    }

    /// True when the caller cancelled the call.
    pub fn is_cancelled(&self) -> bool {
        self.code == codes::IPC_CANCELLED
    }

    /// True when this code is one this build knows about.
    ///
    /// A `false` here means the bridge is newer than the client; treat the
    /// error as a generic bridge failure rather than guessing.
    pub fn is_known_code(&self) -> bool {
        codes::is_known(&self.code)
    }

    /// Wraps a local `std::io::Error` against a path inside the IPC directory.
    pub fn io(context: &str, path: &std::path::Path, err: &std::io::Error) -> IpcError {
        let mut m = JsonMap::new();
        m.insert("path", Json::Str(path.display().to_string()));
        m.insert("os_error", Json::Str(err.kind_string()));
        IpcError::with_details(
            codes::IPC_IO_ERROR,
            format!("{context}: {err}"),
            Json::Obj(m),
        )
    }
}

/// Small helper so [`IpcError::io`] does not depend on `io::ErrorKind`'s
/// `Debug` formatting being stable.
trait KindString {
    fn kind_string(&self) -> String;
}

impl KindString for std::io::Error {
    fn kind_string(&self) -> String {
        match self.kind() {
            std::io::ErrorKind::NotFound => "not_found".to_string(),
            std::io::ErrorKind::PermissionDenied => "permission_denied".to_string(),
            std::io::ErrorKind::AlreadyExists => "already_exists".to_string(),
            std::io::ErrorKind::InvalidData => "invalid_data".to_string(),
            other => format!("{other:?}"),
        }
    }
}

fn truncate(mut s: String) -> String {
    if s.len() <= crate::limits::MAX_ERROR_MESSAGE_BYTES {
        return s;
    }
    let mut cut = crate::limits::MAX_ERROR_MESSAGE_BYTES - 3;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
    s.push_str("...");
    s
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.message.is_empty() {
            write!(f, "{}", self.code)
        } else {
            write!(f, "{}: {}", self.code, self.message)
        }
    }
}

impl std::error::Error for IpcError {}

#[cfg(test)]
mod tests {
    use super::*;
    use qjson::json_obj;

    #[test]
    fn vocabulary_matches_the_recorded_bridge_manifest() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/mock-reaper/bridge/error-codes.json"
        );
        let text = std::fs::read_to_string(path).expect("error-codes fixture");
        let doc = Json::parse(&text).expect("parses");
        let recorded: Vec<&str> = doc
            .arr_field("codes")
            .expect("codes")
            .iter()
            .map(|v| v.as_str().expect("string"))
            .collect();
        assert_eq!(recorded, codes::BRIDGE_VOCABULARY);
    }

    #[test]
    fn recorded_client_only_codes_are_a_subset_of_ours() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/mock-reaper/bridge/error-codes.json"
        );
        let text = std::fs::read_to_string(path).expect("error-codes fixture");
        let doc = Json::parse(&text).expect("parses");
        for v in doc.arr_field("client_only").expect("client_only") {
            let c = v.as_str().expect("string");
            assert!(
                codes::CLIENT_ONLY.contains(&c),
                "{c} missing from CLIENT_ONLY"
            );
        }
    }

    #[test]
    fn client_only_and_vocabulary_lists_are_sorted() {
        let mut a = codes::BRIDGE_VOCABULARY.to_vec();
        a.sort_unstable();
        assert_eq!(a, codes::BRIDGE_VOCABULARY);
        let mut b = codes::CLIENT_ONLY.to_vec();
        b.sort_unstable();
        assert_eq!(b, codes::CLIENT_ONLY);
    }

    #[test]
    fn from_wire_preserves_code_and_details_unflattened() {
        let wire = json_obj! {
            "code" => "MIDI_CHANGED",
            "message" => "the take changed",
            "details" => json_obj!{ "expected" => "fnv1a64:aaaa", "actual" => "fnv1a64:bbbb" },
        };
        let e = IpcError::from_wire(&wire);
        assert_eq!(e.code, codes::MIDI_CHANGED);
        assert_eq!(e.detail_str("expected"), Some("fnv1a64:aaaa"));
        assert_eq!(e.detail_str("actual"), Some("fnv1a64:bbbb"));
        assert!(e.is_stale());
    }

    #[test]
    fn from_wire_keeps_unknown_codes_verbatim() {
        let wire = json_obj! { "code" => "SOME_FUTURE_CODE", "message" => "x" };
        let e = IpcError::from_wire(&wire);
        assert_eq!(e.code, "SOME_FUTURE_CODE");
        assert!(!e.is_known_code());
    }

    #[test]
    fn from_wire_normalises_a_non_object_details() {
        let wire = json_obj! { "code" => "X", "message" => "y", "details" => "oops" };
        let e = IpcError::from_wire(&wire);
        assert_eq!(e.details.get("value").and_then(Json::as_str), Some("oops"));
    }

    #[test]
    fn from_wire_on_a_non_object_is_an_internal_bridge_error() {
        let e = IpcError::from_wire(&Json::Str("nope".into()));
        assert_eq!(e.code, codes::INTERNAL_BRIDGE_ERROR);
    }

    #[test]
    fn predicates_are_mutually_consistent() {
        assert!(IpcError::new(codes::BRIDGE_OFFLINE, "").is_bridge_offline());
        assert!(IpcError::new(codes::IPC_TIMEOUT, "").is_timeout());
        assert!(IpcError::new(codes::IPC_CANCELLED, "").is_cancelled());
        for c in [
            codes::STALE_SNAPSHOT,
            codes::PROJECT_CHANGED,
            codes::MIDI_CHANGED,
            codes::TEMPO_MAP_CHANGED,
        ] {
            assert!(IpcError::new(c, "").is_stale(), "{c}");
        }
        assert!(!IpcError::new(codes::UNKNOWN_COMMAND, "").is_stale());
    }

    #[test]
    fn messages_are_capped_at_two_thousand_bytes() {
        let e = IpcError::new("X", "y".repeat(5000));
        assert_eq!(e.message.len(), crate::limits::MAX_ERROR_MESSAGE_BYTES);
        assert!(e.message.ends_with("..."));
    }

    #[test]
    fn display_and_error_impls_are_present() {
        let e = IpcError::new(codes::IPC_TIMEOUT, "no result");
        assert_eq!(e.to_string(), "IPC_TIMEOUT: no result");
        assert_eq!(
            IpcError::new(codes::IPC_TIMEOUT, "").to_string(),
            "IPC_TIMEOUT"
        );
        let _: &dyn std::error::Error = &e;
    }

    #[test]
    fn round_trips_through_the_wire_form() {
        let e = IpcError::with_details(
            codes::STALE_SNAPSHOT,
            "stale",
            json_obj! { "expected" => "a" },
        );
        assert_eq!(IpcError::from_wire(&e.to_json()), e);
    }
}
