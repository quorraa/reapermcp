//! The single error type produced by loading, validating and querying the
//! knowledge bundle.
//!
//! [`KbError`] is deliberately flat: a stable machine-readable `code`, a
//! human-navigable `path` naming the offending file and record, and a
//! `message`. Nothing in this crate panics on bad input; every fallible entry
//! point returns this type.

use qjson::{Json, JsonError, JsonMap};

/// A knowledge-base failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KbError {
    /// Stable machine-readable code, e.g. `"KB_DUPLICATE_ID"`.
    pub code: String,
    /// Where the problem is: `"knowledge/rules/harmony.json#items[3].conditions[0]"`.
    pub path: String,
    /// What is wrong, in a sentence.
    pub message: String,
}

impl KbError {
    /// Builds an error from its three parts.
    pub fn new(
        code: impl Into<String>,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        KbError {
            code: code.into(),
            path: path.into(),
            message: message.into(),
        }
    }

    /// JSON form, used by `xtask --json` and by the MCP error payloads.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("code", Json::Str(self.code.clone()));
        m.insert("path", Json::Str(self.path.clone()));
        m.insert("message", Json::Str(self.message.clone()));
        Json::Obj(m)
    }
}

/// Declares the stable error-code constructors so each call site reads as a
/// sentence rather than as a string literal.
macro_rules! kb_error_kinds {
    ($( $(#[$doc:meta])* $name:ident => $code:literal ),+ $(,)?) => {
        impl KbError {
            $(
                $(#[$doc])*
                pub fn $name(path: impl Into<String>, message: impl Into<String>) -> KbError {
                    KbError::new($code, path, message)
                }
            )+
        }
    };
}

kb_error_kinds! {
    /// A file could not be read from disk.
    io => "KB_IO",
    /// A knowledge file is not well-formed JSON.
    parse => "KB_PARSE",
    /// A knowledge file failed its JSON Schema.
    schema => "KB_SCHEMA",
    /// A required field is missing or has the wrong type.
    shape => "KB_SHAPE",
    /// Two records in the same collection share an id.
    duplicate_id => "KB_DUPLICATE_ID",
    /// A record references an id that does not exist.
    unresolved_ref => "KB_UNRESOLVED_REF",
    /// Profile inheritance forms a cycle.
    profile_cycle => "KB_PROFILE_CYCLE",
    /// A profile names a parent that does not exist.
    missing_parent => "KB_MISSING_PARENT",
    /// A rule uses a predicate this build cannot evaluate.
    unknown_predicate => "KB_UNKNOWN_PREDICATE",
    /// An enumerated field carries a value outside its closed vocabulary.
    unknown_enum => "KB_UNKNOWN_ENUM",
    /// `score_weights` does not cover exactly the thirteen score components.
    invalid_weights => "KB_INVALID_WEIGHTS",
    /// A degree string does not match `^[b#]{0,2}\d+$`.
    invalid_degree => "KB_INVALID_DEGREE",
    /// A rule declares no `test_ids`.
    missing_test_ids => "KB_MISSING_TEST_IDS",
    /// The manifest's `content_sha256` does not match the files on disk.
    hash_mismatch => "KB_HASH_MISMATCH",
    /// The manifest still reads `"PENDING"` where a real hash is required.
    unstamped_manifest => "KB_UNSTAMPED_MANIFEST",
    /// The manifest's declared counts disagree with the loaded records.
    count_mismatch => "KB_COUNT_MISMATCH",
    /// A required knowledge file is absent from the bundle.
    missing_file => "KB_MISSING_FILE",
    /// A record breaks an invariant that spans several fields.
    inconsistent => "KB_INCONSISTENT",
    /// A lookup by id found nothing.
    not_found => "KB_NOT_FOUND",
}

impl std::fmt::Display for KbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            write!(f, "[{}] {}", self.code, self.message)
        } else {
            write!(f, "[{}] {}: {}", self.code, self.path, self.message)
        }
    }
}

impl std::error::Error for KbError {}

impl From<JsonError> for KbError {
    fn from(e: JsonError) -> KbError {
        KbError::shape(e.path.clone(), e.message.clone())
    }
}

impl From<music_domain::DomainError> for KbError {
    fn from(e: music_domain::DomainError) -> KbError {
        KbError::new(format!("KB_DOMAIN_{}", e.code), "", e.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_includes_path_when_present() {
        let e = KbError::duplicate_id("knowledge/scales.json", "id 'major' appears twice");
        assert_eq!(
            e.to_string(),
            "[KB_DUPLICATE_ID] knowledge/scales.json: id 'major' appears twice"
        );
    }

    #[test]
    fn display_omits_empty_path() {
        let e = KbError::not_found("", "no such profile");
        assert_eq!(e.to_string(), "[KB_NOT_FOUND] no such profile");
    }

    #[test]
    fn json_form_is_stable() {
        let e = KbError::schema("a", "b");
        assert_eq!(
            e.to_json().to_string(),
            r#"{"code":"KB_SCHEMA","path":"a","message":"b"}"#
        );
    }

    #[test]
    fn json_error_converts() {
        let je = Json::Null.str_field("x").unwrap_err();
        let ke: KbError = je.into();
        assert_eq!(ke.code, "KB_SHAPE");
    }

    #[test]
    fn domain_error_converts() {
        let de = music_domain::DomainError::new("BAD", "nope");
        let ke: KbError = de.into();
        assert_eq!(ke.code, "KB_DOMAIN_BAD");
        assert_eq!(ke.message, "nope");
    }
}
