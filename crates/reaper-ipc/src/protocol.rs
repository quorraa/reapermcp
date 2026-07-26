//! Request-id safety and a faithful port of the bridge's envelope validation.
//!
//! [`validate_request_id`] is the security-relevant one: `request_id` is the
//! only caller-influenced value that reaches a filesystem path, and it is
//! checked against the wire pattern *before* any path is constructed.
//!
//! [`validate_envelope`] reproduces the bridge's 20-step gauntlet in the
//! normative order, returning on the first failure with the same code the
//! bridge would return. It serves two purposes: the client can check its own
//! envelope before writing it, and [`crate::testing::FakeBridge`] can reject
//! envelopes exactly as the real bridge does, so the error-path tests exercise
//! real ordering rather than an invented one.

use crate::error::{codes, IpcError};
use crate::limits;
use qjson::Json;

/// The request-id pattern: `^[A-Za-z0-9][A-Za-z0-9._-]*$`, 1..=128 bytes, with
/// no `..` substring.
pub const ID_PATTERN: &str = "^[A-Za-z0-9][A-Za-z0-9._-]*$";

/// True when `s` is a safe filesystem-facing identifier.
///
/// Deliberately excludes `/`, `\`, `:` and any `..` run, so no IPC-supplied
/// value can name a path outside the configured IPC directory.
pub fn is_safe_id(s: &str, max_len: usize) -> bool {
    if s.is_empty() || s.len() > max_len {
        return false;
    }
    let b = s.as_bytes();
    if !b[0].is_ascii_alphanumeric() {
        return false;
    }
    if !b
        .iter()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    {
        return false;
    }
    if s.contains("..") {
        return false;
    }
    true
}

/// Validates a `request_id` against the wire pattern.
///
/// This is the gate that stops a caller-supplied value from escaping the IPC
/// directory. Call it before building any path from `request_id`.
pub fn validate_request_id(id: &str) -> Result<(), IpcError> {
    if is_safe_id(id, limits::MAX_REQUEST_ID_LEN) {
        return Ok(());
    }
    Err(IpcError::with_details(
        codes::MALFORMED_REQUEST,
        format!(
            "request_id must match {ID_PATTERN}, be 1-{} bytes and contain no \"..\"",
            limits::MAX_REQUEST_ID_LEN
        ),
        qjson::json_obj! { "request_id" => id, "pattern" => ID_PATTERN },
    ))
}

/// What [`validate_envelope`] needs to know about its surroundings.
#[derive(Clone, Debug)]
pub struct ValidationContext<'a> {
    /// The configured installation token. An empty token rejects everything.
    pub instance_token: &'a str,
    /// Current Unix time, in seconds.
    pub now: i64,
    /// The filename stem the envelope must agree with, when known.
    pub request_id_hint: Option<&'a str>,
    /// Request ids already processed by this bridge instance.
    pub seen: &'a [String],
    /// The size of the command file in bytes, when known.
    pub raw_size: Option<usize>,
    /// The bridge version `require_bridge_version` is compared against.
    pub bridge_version: &'a str,
}

impl<'a> ValidationContext<'a> {
    /// A context with no replay history and no size information.
    pub fn new(instance_token: &'a str, now: i64) -> ValidationContext<'a> {
        ValidationContext {
            instance_token,
            now,
            request_id_hint: None,
            seen: &[],
            raw_size: None,
            bridge_version: limits::BRIDGE_VERSION,
        }
    }
}

/// A request that passed every envelope-level check.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedRequest {
    /// The validated id.
    pub request_id: String,
    /// The allowlisted command.
    pub command: String,
    /// `expires_at` as Unix seconds.
    pub expires_unix: i64,
    /// The payload, normalised to an object.
    pub payload: Json,
    /// The precondition block, when supplied.
    pub expected_project: Option<Json>,
    /// True when this command needs an active REAPER project.
    pub needs_project: bool,
}

/// Reads a string field, treating JSON `null` and absence identically.
///
/// Mirrors the bridge, which also treats a *wrong-typed* value as absent rather
/// than as an error. That leniency is load-bearing: matching it is what makes
/// the fake bridge's rejections identical to the real one's.
fn opt_string<'a>(v: &'a Json, key: &str) -> Option<&'a str> {
    match v.get(key) {
        Some(Json::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

fn is_object(v: Option<&Json>) -> bool {
    matches!(v, Some(Json::Obj(_)))
}

/// Runs the bridge's validation gauntlet in the normative §3 order.
///
/// Returns on the first failure. The step numbers in the source comments are
/// the row numbers of the §3 table.
pub fn validate_envelope(
    req: &Json,
    ctx: &ValidationContext<'_>,
) -> Result<ValidatedRequest, IpcError> {
    // 1. file size
    if let Some(size) = ctx.raw_size {
        if size > limits::MAX_REQUEST_BYTES {
            return Err(IpcError::with_details(
                codes::PAYLOAD_TOO_LARGE,
                format!(
                    "request is {size} bytes; limit is {}",
                    limits::MAX_REQUEST_BYTES
                ),
                qjson::json_obj! { "size" => size, "limit" => limits::MAX_REQUEST_BYTES },
            ));
        }
    }

    // 2-3. body parses as JSON (the caller has already done this) and is an object
    if req.as_obj().is_none() {
        return Err(IpcError::new(
            codes::MALFORMED_REQUEST,
            "request envelope must be a JSON object",
        ));
    }

    // 4. protocol_version
    let pv = opt_string(req, "protocol_version");
    match pv {
        None => {
            return Err(IpcError::new(
                codes::IPC_PROTOCOL_MISMATCH,
                "missing protocol_version",
            ))
        }
        Some(v) if v != limits::PROTOCOL_VERSION => {
            return Err(IpcError::with_details(
                codes::IPC_PROTOCOL_MISMATCH,
                format!(
                    "protocol_version {v:?} is not {:?}",
                    limits::PROTOCOL_VERSION
                ),
                qjson::json_obj! { "expected" => limits::PROTOCOL_VERSION, "received" => v },
            ));
        }
        Some(_) => {}
    }

    // 5. require_bridge_version
    if let Some(want) = opt_string(req, "require_bridge_version") {
        if want != ctx.bridge_version {
            return Err(IpcError::with_details(
                codes::BRIDGE_VERSION_MISMATCH,
                format!("bridge is {}; caller requires {want}", ctx.bridge_version),
                qjson::json_obj! { "bridge_version" => ctx.bridge_version, "required" => want },
            ));
        }
    }

    // 6. instance_token
    let token = opt_string(req, "instance_token").unwrap_or("");
    if token.is_empty() {
        return Err(IpcError::new(
            codes::INVALID_INSTANCE_TOKEN,
            "missing instance_token",
        ));
    }
    if ctx.instance_token.is_empty() {
        return Err(IpcError::new(
            codes::INVALID_INSTANCE_TOKEN,
            "bridge has no configured instance token",
        ));
    }
    if !tokens_equal(token, ctx.instance_token) {
        return Err(IpcError::new(
            codes::INVALID_INSTANCE_TOKEN,
            "instance_token does not match this installation",
        ));
    }

    // 7. request_id pattern
    let rid = opt_string(req, "request_id").unwrap_or("");
    validate_request_id(rid)?;

    // 8. request_id equals the filename stem
    if let Some(stem) = ctx.request_id_hint {
        if rid != stem {
            return Err(IpcError::with_details(
                codes::MALFORMED_REQUEST,
                format!("request_id {rid:?} does not match filename stem {stem:?}"),
                qjson::json_obj! { "request_id" => rid, "stem" => stem },
            ));
        }
    }

    // 9. replay guard
    if ctx.seen.iter().any(|s| s == rid) {
        return Err(IpcError::with_details(
            codes::DUPLICATE_REQUEST,
            format!("request_id {rid:?} was already processed"),
            qjson::json_obj! { "request_id" => rid },
        ));
    }

    // 10. expires_at present and parseable
    let Some(expires) = opt_string(req, "expires_at") else {
        return Err(IpcError::new(
            codes::MALFORMED_REQUEST,
            "missing expires_at",
        ));
    };
    let Some(exp) = qjson::time::parse_iso8601(expires) else {
        return Err(IpcError::new(
            codes::MALFORMED_REQUEST,
            "expires_at is not a supported ISO-8601 timestamp",
        ));
    };

    // 11. expiry
    if ctx.now > exp + limits::CLOCK_SKEW_SECONDS {
        return Err(IpcError::with_details(
            codes::EXPIRED_REQUEST,
            format!(
                "request expired at {expires} (now {})",
                qjson::time::iso8601_from_unix(ctx.now)
            ),
            qjson::json_obj! {
                "expires_at" => expires,
                "now" => qjson::time::iso8601_from_unix(ctx.now),
            },
        ));
    }

    // 12. created_at parseable when present
    if let Some(created) = opt_string(req, "created_at") {
        if qjson::time::parse_iso8601(created).is_none() {
            return Err(IpcError::new(
                codes::MALFORMED_REQUEST,
                "created_at is not a supported ISO-8601 timestamp",
            ));
        }
    }

    // 13. command present
    let Some(cmd) = opt_string(req, "command") else {
        return Err(IpcError::new(codes::MALFORMED_REQUEST, "missing command"));
    };

    // 14. command allowlisted
    if !limits::is_allowed_command(cmd) {
        let shown: String = cmd.chars().take(64).collect();
        return Err(IpcError::with_details(
            codes::UNKNOWN_COMMAND,
            format!("command {shown:?} is not allowlisted"),
            qjson::json_obj! {
                "allowed" => Json::Arr(limits::COMMANDS.iter().map(|c| Json::Str((*c).to_string())).collect()),
            },
        ));
    }

    // 15. payload is an object or null
    let payload = match req.get("payload") {
        None | Some(Json::Null) => Json::Obj(qjson::JsonMap::new()),
        Some(Json::Obj(m)) => Json::Obj(m.clone()),
        Some(_) => {
            return Err(IpcError::new(
                codes::MALFORMED_REQUEST,
                "payload must be a JSON object",
            ))
        }
    };

    // 16. expected_project is an object or null
    let expected = match req.get("expected_project") {
        None | Some(Json::Null) => None,
        Some(v) if is_object(Some(v)) => Some(v.clone()),
        Some(_) => {
            return Err(IpcError::new(
                codes::MALFORMED_REQUEST,
                "expected_project must be a JSON object or null",
            ))
        }
    };

    // 17-20 are host- and command-specific and belong to the bridge.
    Ok(ValidatedRequest {
        request_id: rid.to_string(),
        command: cmd.to_string(),
        expires_unix: exp,
        payload,
        expected_project: expected,
        needs_project: limits::command_needs_project(cmd),
    })
}

/// Byte-length-sensitive, prefix-non-leaking token comparison.
///
/// The token is not a security boundary — the wire spec says so explicitly —
/// but there is no reason to leak a prefix match either.
fn tokens_equal(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";
    /// 2026-07-26T18:51:20Z — the instant the fixture README pins.
    const NOW: i64 = 1_785_091_880;

    fn request(name: &str) -> Json {
        let path = format!(
            "{}/../../fixtures/mock-reaper/requests/{name}.command.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        Json::parse(&text).expect("parses")
    }

    fn ctx_for(stem: &str) -> ValidationContext<'static> {
        // Leaks a stem string for the lifetime of the test; harmless and keeps
        // the call sites readable.
        let stem: &'static str = Box::leak(stem.to_string().into_boxed_str());
        ValidationContext {
            instance_token: TOKEN,
            now: NOW,
            request_id_hint: Some(stem),
            seen: &[],
            raw_size: None,
            bridge_version: limits::BRIDGE_VERSION,
        }
    }

    #[test]
    fn the_pinned_now_matches_the_fixture_readme() {
        assert_eq!(qjson::time::iso8601_from_unix(NOW), "2026-07-26T18:51:20Z");
    }

    #[test]
    fn safe_ids_accept_the_recommended_uuid_shape() {
        assert!(is_safe_id("a3f0c1d2-1111-4222-8333-444455556666", 128));
        assert!(is_safe_id("valid-ping", 128));
        assert!(is_safe_id("a", 128));
        assert!(is_safe_id("A0._-", 128));
    }

    #[test]
    fn safe_ids_reject_path_escapes_and_separators() {
        for bad in [
            "../../etc/passwd",
            "..",
            "a..b",
            "a/b",
            "a\\b",
            "C:\\x",
            "/abs",
            "./x",
            "",
            ".hidden",
            "-leading",
            "_leading",
            "a b",
            "a\0b",
            "a\nb",
            "é",
        ] {
            assert!(!is_safe_id(bad, 128), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn safe_ids_enforce_the_length_cap_in_bytes() {
        let ok = "a".repeat(128);
        let too_long = "a".repeat(129);
        assert!(is_safe_id(&ok, 128));
        assert!(!is_safe_id(&too_long, 128));
    }

    #[test]
    fn validate_request_id_reports_malformed_request_with_the_pattern() {
        let err = validate_request_id("../x").expect_err("escape");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
        assert_eq!(err.detail_str("pattern"), Some(ID_PATTERN));
    }

    #[test]
    fn valid_ping_fixture_is_accepted() {
        let v =
            validate_envelope(&request("valid-ping"), &ctx_for("valid-ping")).expect("accepted");
        assert_eq!(v.command, "ping");
        assert_eq!(v.request_id, "valid-ping");
        assert!(!v.needs_project);
    }

    #[test]
    fn valid_status_fixture_is_accepted() {
        let v = validate_envelope(&request("valid-status"), &ctx_for("valid-status"))
            .expect("accepted");
        assert_eq!(v.command, "status");
        assert!(!v.needs_project);
    }

    #[test]
    fn valid_inspect_fixture_is_accepted_and_needs_a_project() {
        let v = validate_envelope(
            &request("valid-inspect-selection"),
            &ctx_for("valid-inspect-selection"),
        )
        .expect("accepted");
        assert_eq!(v.command, "inspect_selection");
        assert!(v.needs_project);
        assert_eq!(
            v.payload.get("note_scope").and_then(Json::as_str),
            Some("selected_or_all")
        );
    }

    #[test]
    fn valid_inspect_with_preconditions_carries_expected_project_through() {
        let v = validate_envelope(
            &request("valid-inspect-with-preconditions"),
            &ctx_for("valid-inspect-with-preconditions"),
        )
        .expect("accepted");
        let e = v.expected_project.expect("expected_project");
        assert_eq!(
            e.get("snapshot_hash").and_then(Json::as_str),
            Some("fnv1a64:3e7bf035037bcf4f")
        );
    }

    #[test]
    fn valid_transaction_fixtures_are_accepted() {
        for (stem, cmd) in [
            ("valid-commit", "commit_candidate"),
            ("valid-discard", "discard_candidate"),
            ("valid-undo", "undo_last_generation"),
        ] {
            let v = validate_envelope(&request(stem), &ctx_for(stem)).expect("accepted");
            assert_eq!(v.command, cmd);
            assert_eq!(
                v.payload.get("transaction_id").and_then(Json::as_str),
                Some("tx-0001")
            );
        }
    }

    #[test]
    fn invalid_protocol_version_fixture_is_a_protocol_mismatch() {
        let err = validate_envelope(
            &request("invalid-protocol-version"),
            &ctx_for("invalid-protocol-version"),
        )
        .expect_err("rejected");
        assert_eq!(err.code, codes::IPC_PROTOCOL_MISMATCH);
        assert_eq!(err.detail_str("received"), Some("qlabs-reaper-ipc/999"));
    }

    #[test]
    fn invalid_token_fixture_is_an_invalid_instance_token() {
        let err = validate_envelope(&request("invalid-token"), &ctx_for("invalid-token"))
            .expect_err("rejected");
        assert_eq!(err.code, codes::INVALID_INSTANCE_TOKEN);
    }

    #[test]
    fn invalid_expired_fixture_is_an_expired_request() {
        let err = validate_envelope(&request("invalid-expired"), &ctx_for("invalid-expired"))
            .expect_err("rejected");
        assert_eq!(err.code, codes::EXPIRED_REQUEST);
        assert_eq!(err.detail_str("expires_at"), Some("2020-01-01T00:00:00Z"));
    }

    #[test]
    fn invalid_unknown_command_fixture_is_an_unknown_command() {
        let err = validate_envelope(
            &request("invalid-unknown-command"),
            &ctx_for("invalid-unknown-command"),
        )
        .expect_err("rejected");
        assert_eq!(err.code, codes::UNKNOWN_COMMAND);
        let allowed = err
            .details
            .get("allowed")
            .and_then(Json::as_arr)
            .expect("allowed");
        assert_eq!(allowed.len(), limits::COMMANDS.len());
    }

    #[test]
    fn invalid_request_id_fixture_is_a_malformed_request() {
        // No stem hint: the id itself is what fails, at step 7.
        let ctx = ValidationContext::new(TOKEN, NOW);
        let err = validate_envelope(&request("invalid-request-id"), &ctx).expect_err("rejected");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
        assert_eq!(err.detail_str("request_id"), Some("../../etc/passwd"));
    }

    #[test]
    fn invalid_payload_is_array_fixture_is_a_malformed_request() {
        let err = validate_envelope(
            &request("invalid-payload-is-array"),
            &ctx_for("invalid-payload-is-array"),
        )
        .expect_err("rejected");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
    }

    #[test]
    fn the_truncated_fixture_is_not_valid_json_at_all() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/mock-reaper/requests/invalid-truncated.command.json"
        );
        let text = std::fs::read_to_string(path).expect("fixture");
        assert!(Json::parse(&text).is_err());
    }

    #[test]
    fn a_request_id_that_differs_from_the_stem_is_rejected() {
        let err = validate_envelope(&request("valid-ping"), &ctx_for("some-other-stem"))
            .expect_err("rejected");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
        assert_eq!(err.detail_str("stem"), Some("some-other-stem"));
    }

    #[test]
    fn a_duplicate_request_id_is_rejected() {
        let seen = vec!["valid-ping".to_string()];
        let mut ctx = ctx_for("valid-ping");
        ctx.seen = &seen;
        let err = validate_envelope(&request("valid-ping"), &ctx).expect_err("duplicate");
        assert_eq!(err.code, codes::DUPLICATE_REQUEST);
        assert_eq!(err.detail_str("request_id"), Some("valid-ping"));
    }

    #[test]
    fn an_oversized_request_is_rejected_before_anything_else() {
        // Deliberately also broken in three other ways; size must still win.
        let doc = qjson::json_obj! {
            "protocol_version" => "nope",
            "request_id" => "../escape",
            "instance_token" => "wrong",
            "command" => "execute_lua",
        };
        let mut ctx = ctx_for("x");
        ctx.raw_size = Some(limits::MAX_REQUEST_BYTES + 1);
        let err = validate_envelope(&doc, &ctx).expect_err("too large");
        assert_eq!(err.code, codes::PAYLOAD_TOO_LARGE);
        assert_eq!(
            err.details.get("limit").and_then(Json::as_i64),
            Some(limits::MAX_REQUEST_BYTES as i64)
        );
    }

    #[test]
    fn a_request_exactly_at_the_size_limit_is_accepted() {
        let mut ctx = ctx_for("valid-ping");
        ctx.raw_size = Some(limits::MAX_REQUEST_BYTES);
        validate_envelope(&request("valid-ping"), &ctx).expect("at the limit");
    }

    #[test]
    fn require_bridge_version_is_checked_before_the_token() {
        let doc = qjson::json_obj! {
            "protocol_version" => limits::PROTOCOL_VERSION,
            "require_bridge_version" => "9.9.9",
            "instance_token" => "definitely-wrong",
            "request_id" => "x",
            "expires_at" => "2026-07-26T18:51:49Z",
            "command" => "ping",
        };
        let err = validate_envelope(&doc, &ctx_for("x")).expect_err("rejected");
        assert_eq!(err.code, codes::BRIDGE_VERSION_MISMATCH);
    }

    #[test]
    fn a_matching_require_bridge_version_passes() {
        let doc = qjson::json_obj! {
            "protocol_version" => limits::PROTOCOL_VERSION,
            "require_bridge_version" => limits::BRIDGE_VERSION,
            "instance_token" => TOKEN,
            "request_id" => "x",
            "expires_at" => "2026-07-26T18:51:49Z",
            "command" => "ping",
        };
        validate_envelope(&doc, &ctx_for("x")).expect("accepted");
    }

    #[test]
    fn a_bridge_with_no_configured_token_rejects_everything() {
        let mut ctx = ctx_for("valid-ping");
        ctx.instance_token = "";
        let err = validate_envelope(&request("valid-ping"), &ctx).expect_err("rejected");
        assert_eq!(err.code, codes::INVALID_INSTANCE_TOKEN);
    }

    #[test]
    fn a_token_of_the_wrong_length_is_rejected() {
        assert!(!tokens_equal("abc", "abcd"));
        assert!(!tokens_equal("abcd", "abc"));
        assert!(tokens_equal("abcd", "abcd"));
        assert!(tokens_equal("", ""));
    }

    #[test]
    fn expiry_is_tolerant_by_exactly_the_clock_skew() {
        let expires = qjson::time::parse_iso8601("2026-07-26T18:51:49Z").expect("parse");
        let doc = request("valid-ping");
        for (now, ok) in [
            (expires, true),
            (expires + limits::CLOCK_SKEW_SECONDS, true),
            (expires + limits::CLOCK_SKEW_SECONDS + 1, false),
        ] {
            let mut ctx = ctx_for("valid-ping");
            ctx.now = now;
            let got = validate_envelope(&doc, &ctx);
            assert_eq!(got.is_ok(), ok, "now={now}");
            if !ok {
                assert_eq!(got.expect_err("expired").code, codes::EXPIRED_REQUEST);
            }
        }
    }

    #[test]
    fn a_missing_expires_at_is_a_malformed_request() {
        let doc = qjson::json_obj! {
            "protocol_version" => limits::PROTOCOL_VERSION,
            "instance_token" => TOKEN,
            "request_id" => "x",
            "command" => "ping",
        };
        let err = validate_envelope(&doc, &ctx_for("x")).expect_err("rejected");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
    }

    #[test]
    fn an_unparseable_expires_at_is_a_malformed_request() {
        let doc = qjson::json_obj! {
            "protocol_version" => limits::PROTOCOL_VERSION,
            "instance_token" => TOKEN,
            "request_id" => "x",
            "expires_at" => "yesterday",
            "command" => "ping",
        };
        let err = validate_envelope(&doc, &ctx_for("x")).expect_err("rejected");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
    }

    #[test]
    fn an_unparseable_created_at_is_a_malformed_request() {
        let doc = qjson::json_obj! {
            "protocol_version" => limits::PROTOCOL_VERSION,
            "instance_token" => TOKEN,
            "request_id" => "x",
            "created_at" => "nope",
            "expires_at" => "2026-07-26T18:51:49Z",
            "command" => "ping",
        };
        let err = validate_envelope(&doc, &ctx_for("x")).expect_err("rejected");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
    }

    #[test]
    fn a_missing_command_is_a_malformed_request_not_an_unknown_command() {
        let doc = qjson::json_obj! {
            "protocol_version" => limits::PROTOCOL_VERSION,
            "instance_token" => TOKEN,
            "request_id" => "x",
            "expires_at" => "2026-07-26T18:51:49Z",
        };
        let err = validate_envelope(&doc, &ctx_for("x")).expect_err("rejected");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
    }

    #[test]
    fn a_non_object_envelope_is_a_malformed_request() {
        let err = validate_envelope(&Json::Arr(vec![]), &ctx_for("x")).expect_err("rejected");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
    }

    #[test]
    fn a_missing_protocol_version_is_a_protocol_mismatch() {
        let err = validate_envelope(&qjson::json_obj! { "command" => "ping" }, &ctx_for("x"))
            .expect_err("rejected");
        assert_eq!(err.code, codes::IPC_PROTOCOL_MISMATCH);
    }

    #[test]
    fn an_expected_project_array_is_a_malformed_request() {
        let doc = qjson::json_obj! {
            "protocol_version" => limits::PROTOCOL_VERSION,
            "instance_token" => TOKEN,
            "request_id" => "x",
            "expires_at" => "2026-07-26T18:51:49Z",
            "command" => "ping",
            "expected_project" => Json::Arr(vec![]),
        };
        let err = validate_envelope(&doc, &ctx_for("x")).expect_err("rejected");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
    }

    #[test]
    fn a_null_payload_and_a_null_expected_project_are_treated_as_absent() {
        let doc = qjson::json_obj! {
            "protocol_version" => limits::PROTOCOL_VERSION,
            "instance_token" => TOKEN,
            "request_id" => "x",
            "expires_at" => "2026-07-26T18:51:49Z",
            "command" => "ping",
            "payload" => Json::Null,
            "expected_project" => Json::Null,
        };
        let v = validate_envelope(&doc, &ctx_for("x")).expect("accepted");
        assert_eq!(v.payload, Json::Obj(qjson::JsonMap::new()));
        assert!(v.expected_project.is_none());
    }

    #[test]
    fn unknown_top_level_fields_are_ignored() {
        let mut doc = request("valid-ping");
        if let Json::Obj(m) = &mut doc {
            m.insert("something_new", Json::Int(1));
        }
        validate_envelope(&doc, &ctx_for("valid-ping")).expect("accepted");
    }
}
