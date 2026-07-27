//! Building request envelopes and reading result envelopes.
//!
//! The request side is deliberately narrow: [`RequestEnvelope::build`] is the
//! only way this crate produces a document destined for `commands/`, and it
//! validates the `request_id` against the wire pattern before that id can reach
//! a path.
//!
//! # Serialisation
//!
//! [`serialize`] emits the same bytes the bridge does: 2-space pretty JSON,
//! object keys sorted ascending by byte value, and a trailing newline. Nothing
//! *requires* the client to sort — §0 of the wire spec forbids either side from
//! relying on key order when reading — but matching the bridge byte for byte
//! makes recorded envelopes usable as golden files and makes a diff of two
//! captured requests meaningful.

use crate::error::{codes, IpcError};
use crate::limits;
use crate::protocol::validate_request_id;
use qjson::{Json, JsonMap};

/// The `expected_project` precondition block (§5).
///
/// Every field is optional; a field left `None` is **not enforced** by the
/// bridge.
///
/// # Choosing fields
///
/// `state_change_count` increments on any project edit, including a change of
/// item selection, so enforcing it makes a request fail for benign reasons.
/// Prefer `project_uuid` + `snapshot_hash` (or `midi_hash` + `tempo_map_hash`)
/// for staleness, and reserve `state_change_count` for flows where absolutely
/// nothing may have happened.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExpectedProject {
    /// Persistent project UUID. Mismatch is `PROJECT_CHANGED`.
    pub project_uuid: Option<String>,
    /// Exact project state-change counter. Mismatch is `PROJECT_CHANGED`.
    pub state_change_count: Option<i64>,
    /// Item GUID that must resolve. Failure is `SOURCE_ITEM_MISSING`.
    pub source_item_guid: Option<String>,
    /// Take GUID that must resolve. Failure is `SOURCE_TAKE_MISSING`.
    pub source_take_guid: Option<String>,
    /// Take MIDI hash. Mismatch is `MIDI_CHANGED`. Requires `source_take_guid`.
    pub midi_hash: Option<String>,
    /// Tempo-map hash. Mismatch is `TEMPO_MAP_CHANGED`.
    pub tempo_map_hash: Option<String>,
    /// Whole-snapshot hash. Mismatch is `STALE_SNAPSHOT`.
    pub snapshot_hash: Option<String>,
}

impl ExpectedProject {
    /// True when no field is set, in which case the block should be omitted.
    pub fn is_empty(&self) -> bool {
        self.project_uuid.is_none()
            && self.state_change_count.is_none()
            && self.source_item_guid.is_none()
            && self.source_take_guid.is_none()
            && self.midi_hash.is_none()
            && self.tempo_map_hash.is_none()
            && self.snapshot_hash.is_none()
    }

    /// The wire form. Unset fields are omitted rather than sent as `null`;
    /// the bridge treats the two identically, and omitting keeps the envelope
    /// small.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        if let Some(v) = &self.project_uuid {
            m.insert("project_uuid", Json::Str(v.clone()));
        }
        if let Some(v) = self.state_change_count {
            m.insert("state_change_count", Json::Int(v));
        }
        if let Some(v) = &self.source_item_guid {
            m.insert("source_item_guid", Json::Str(v.clone()));
        }
        if let Some(v) = &self.source_take_guid {
            m.insert("source_take_guid", Json::Str(v.clone()));
        }
        if let Some(v) = &self.midi_hash {
            m.insert("midi_hash", Json::Str(v.clone()));
        }
        if let Some(v) = &self.tempo_map_hash {
            m.insert("tempo_map_hash", Json::Str(v.clone()));
        }
        if let Some(v) = &self.snapshot_hash {
            m.insert("snapshot_hash", Json::Str(v.clone()));
        }
        Json::Obj(m)
    }

    /// Parses a wire `expected_project` block. `null` fields are treated as
    /// absent.
    pub fn from_json(v: &Json) -> ExpectedProject {
        let s = |k: &str| v.get(k).and_then(Json::as_str).map(str::to_string);
        ExpectedProject {
            project_uuid: s("project_uuid"),
            state_change_count: v.get("state_change_count").and_then(Json::as_i64),
            source_item_guid: s("source_item_guid"),
            source_take_guid: s("source_take_guid"),
            midi_hash: s("midi_hash"),
            tempo_map_hash: s("tempo_map_hash"),
            snapshot_hash: s("snapshot_hash"),
        }
    }

    /// Normalises a REAPER GUID the way the bridge does before comparison:
    /// surrounding braces stripped, ASCII lowercased.
    pub fn normalize_guid(guid: &str) -> String {
        guid.trim_start_matches('{')
            .trim_end_matches('}')
            .to_ascii_lowercase()
    }
}

/// A request envelope (§3), before serialisation.
#[derive(Clone, Debug, PartialEq)]
pub struct RequestEnvelope {
    /// Always [`limits::PROTOCOL_VERSION`].
    pub protocol_version: String,
    /// Validated id; equals the filename stem.
    pub request_id: String,
    /// The installation token from `config.json`.
    pub instance_token: String,
    /// When the client built the request.
    pub created_at: String,
    /// After this instant plus `CLOCK_SKEW_SECONDS` the bridge refuses.
    pub expires_at: String,
    /// An allowlisted command name.
    pub command: String,
    /// Optional exact bridge-version pin.
    pub require_bridge_version: Option<String>,
    /// Optional precondition block.
    pub expected_project: Option<ExpectedProject>,
    /// The command payload. Always an object.
    pub payload: Json,
}

impl RequestEnvelope {
    /// Builds a validated envelope.
    ///
    /// Fails with `MALFORMED_REQUEST` when `request_id` does not match the wire
    /// pattern, with `UNKNOWN_COMMAND` when `command` is not allowlisted, and
    /// with `MALFORMED_REQUEST` when `payload` is neither an object nor null.
    ///
    /// Validating here rather than at write time means an id that could escape
    /// the IPC directory never reaches path construction.
    pub fn build(
        request_id: &str,
        instance_token: &str,
        command: &str,
        payload: Json,
        expected: Option<&ExpectedProject>,
        created_unix: i64,
        expires_unix: i64,
    ) -> Result<RequestEnvelope, IpcError> {
        validate_request_id(request_id)?;
        if !limits::is_allowed_command(command) {
            return Err(IpcError::with_details(
                codes::UNKNOWN_COMMAND,
                format!("command {command:?} is not allowlisted"),
                qjson::json_obj! {
                    "allowed" => Json::Arr(limits::COMMANDS.iter().map(|c| Json::Str((*c).to_string())).collect()),
                },
            ));
        }
        if instance_token.is_empty() {
            return Err(IpcError::new(
                codes::INVALID_INSTANCE_TOKEN,
                "instance_token is empty; read it from the bridge's config.json",
            ));
        }
        let payload = match payload {
            Json::Null => Json::Obj(JsonMap::new()),
            Json::Obj(m) => Json::Obj(m),
            other => {
                return Err(IpcError::with_details(
                    codes::MALFORMED_REQUEST,
                    "payload must be a JSON object",
                    qjson::json_obj! { "type" => other.type_name() },
                ))
            }
        };
        Ok(RequestEnvelope {
            protocol_version: limits::PROTOCOL_VERSION.to_string(),
            request_id: request_id.to_string(),
            instance_token: instance_token.to_string(),
            created_at: qjson::time::iso8601_from_unix(created_unix),
            expires_at: qjson::time::iso8601_from_unix(expires_unix),
            command: command.to_string(),
            require_bridge_version: None,
            expected_project: expected.filter(|e| !e.is_empty()).cloned(),
            payload,
        })
    }

    /// Pins the bridge version. Off by default: pinning turns every bridge
    /// upgrade into a hard `BRIDGE_VERSION_MISMATCH`.
    pub fn require_bridge_version(mut self, version: &str) -> Self {
        self.require_bridge_version = Some(version.to_string());
        self
    }

    /// The wire form.
    pub fn to_json(&self) -> Json {
        let mut m = JsonMap::new();
        m.insert("protocol_version", Json::Str(self.protocol_version.clone()));
        m.insert("request_id", Json::Str(self.request_id.clone()));
        m.insert("instance_token", Json::Str(self.instance_token.clone()));
        m.insert("created_at", Json::Str(self.created_at.clone()));
        m.insert("expires_at", Json::Str(self.expires_at.clone()));
        m.insert("command", Json::Str(self.command.clone()));
        if let Some(v) = &self.require_bridge_version {
            m.insert("require_bridge_version", Json::Str(v.clone()));
        }
        if let Some(e) = &self.expected_project {
            m.insert("expected_project", e.to_json());
        }
        m.insert("payload", self.payload.clone());
        Json::Obj(m)
    }

    /// The exact bytes to write to `commands/<id>.command.json`.
    pub fn to_bytes(&self) -> Vec<u8> {
        serialize(&self.to_json()).into_bytes()
    }
}

/// Serialises a document the way the bridge does: keys sorted ascending,
/// 2-space indent, trailing newline.
pub fn serialize(value: &Json) -> String {
    let mut v = value.clone();
    v.sort_keys_recursive();
    let mut s = v.to_string_pretty();
    s.push('\n');
    s
}

/// A parsed result envelope (§4).
#[derive(Clone, Debug)]
pub struct ResultEnvelope {
    /// The bridge's protocol version.
    pub protocol_version: String,
    /// Always the filename stem.
    pub request_id: String,
    /// The command, or `None` when the request was unparseable.
    pub command: Option<String>,
    /// The discriminator: exactly one of `result` / `error` is non-null.
    pub ok: bool,
    /// The success payload.
    pub result: Option<Json>,
    /// The failure, with its `code` and `details` intact.
    pub error: Option<IpcError>,
    /// Populated for the four transaction commands, including on failure.
    pub transaction_id: Option<String>,
    /// The bridge's own version.
    pub bridge_version: String,
    /// Raw `GetAppVersion()`.
    pub reaper_version: Option<String>,
    /// When the bridge claimed the request.
    pub started_at: Option<String>,
    /// When the bridge finished.
    pub completed_at: Option<String>,
    /// Wall-clock milliseconds spent inside the bridge.
    pub duration_ms: f64,
    /// Envelope-level warnings.
    ///
    /// In practice the bridge always sends `[]` here and puts real warnings
    /// inside `result.warnings`; do not poll this array for them.
    pub warnings: Vec<Json>,
    /// The unmodified envelope.
    pub raw: Json,
}

impl ResultEnvelope {
    /// Parses a result envelope.
    ///
    /// Enforces the §4 invariants that a client depends on: the body is an
    /// object, `protocol_version` matches, `ok` is present and boolean, and
    /// exactly one of `result` / `error` is non-null.
    pub fn parse(value: &Json) -> Result<ResultEnvelope, IpcError> {
        let obj = value.as_obj().ok_or_else(|| {
            IpcError::new(
                codes::INTERNAL_BRIDGE_ERROR,
                "result envelope is not a JSON object",
            )
        })?;

        let protocol_version = obj
            .get("protocol_version")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string();
        if protocol_version != limits::PROTOCOL_VERSION {
            return Err(IpcError::with_details(
                codes::IPC_PROTOCOL_MISMATCH,
                format!(
                    "result declares protocol {protocol_version:?}, expected {:?}",
                    limits::PROTOCOL_VERSION
                ),
                qjson::json_obj! {
                    "expected" => limits::PROTOCOL_VERSION,
                    "received" => protocol_version.clone(),
                },
            ));
        }

        let ok = obj.get("ok").and_then(Json::as_bool).ok_or_else(|| {
            IpcError::new(
                codes::INTERNAL_BRIDGE_ERROR,
                "result envelope has no boolean `ok` discriminator",
            )
        })?;

        let result = match obj.get("result") {
            Some(Json::Null) | None => None,
            Some(other) => Some(other.clone()),
        };
        let error = match obj.get("error") {
            Some(Json::Null) | None => None,
            Some(other) => Some(IpcError::from_wire(other)),
        };

        if ok && error.is_some() {
            return Err(IpcError::new(
                codes::INTERNAL_BRIDGE_ERROR,
                "result envelope says ok but carries an error object",
            ));
        }
        if !ok && error.is_none() {
            return Err(IpcError::new(
                codes::INTERNAL_BRIDGE_ERROR,
                "result envelope says not-ok but carries no error object",
            ));
        }
        if !ok && result.is_some() {
            return Err(IpcError::new(
                codes::INTERNAL_BRIDGE_ERROR,
                "result envelope says not-ok but carries a result object",
            ));
        }

        let s = |k: &str| obj.get(k).and_then(Json::as_str).map(str::to_string);
        Ok(ResultEnvelope {
            protocol_version,
            request_id: s("request_id").unwrap_or_default(),
            command: s("command"),
            ok,
            result,
            error,
            transaction_id: s("transaction_id"),
            bridge_version: s("bridge_version").unwrap_or_default(),
            reaper_version: s("reaper_version"),
            started_at: s("started_at"),
            completed_at: s("completed_at"),
            duration_ms: obj.get("duration_ms").and_then(Json::as_f64).unwrap_or(0.0),
            warnings: match obj.get("warnings") {
                Some(Json::Arr(a)) => a.clone(),
                _ => Vec::new(),
            },
            raw: value.clone(),
        })
    }

    /// Collapses the envelope into the ordinary Rust outcome.
    ///
    /// A success with no `result` object yields an empty object rather than an
    /// error: the bridge encodes "nothing to say" that way.
    pub fn into_outcome(self) -> Result<Json, IpcError> {
        if self.ok {
            Ok(self.result.unwrap_or_else(|| Json::Obj(JsonMap::new())))
        } else {
            Err(self.error.unwrap_or_else(|| {
                IpcError::new(
                    codes::INTERNAL_BRIDGE_ERROR,
                    "bridge reported an unspecified failure",
                )
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";
    // 2026-07-26T18:51:19Z / 2026-07-26T18:51:49Z, the fixture instants.
    const CREATED: i64 = 1_785_091_879;
    const EXPIRES: i64 = 1_785_091_909;

    fn fixture(name: &str) -> Json {
        let path = format!(
            "{}/../../fixtures/mock-reaper/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        Json::parse(&text).expect("parses")
    }

    #[test]
    fn timestamps_match_the_fixture_instants() {
        assert_eq!(
            qjson::time::iso8601_from_unix(CREATED),
            "2026-07-26T18:51:19Z"
        );
        assert_eq!(
            qjson::time::iso8601_from_unix(EXPIRES),
            "2026-07-26T18:51:49Z"
        );
    }

    #[test]
    fn ping_envelope_matches_the_recorded_fixture_byte_for_byte() {
        let env = RequestEnvelope::build(
            "valid-ping",
            TOKEN,
            "ping",
            Json::Obj(JsonMap::new()),
            None,
            CREATED,
            EXPIRES,
        )
        .expect("build");
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/mock-reaper/requests/valid-ping.command.json"
        );
        let recorded = std::fs::read_to_string(path).expect("fixture");
        assert_eq!(String::from_utf8(env.to_bytes()).expect("utf8"), recorded);
    }

    #[test]
    fn status_envelope_matches_the_recorded_fixture() {
        let env = RequestEnvelope::build(
            "valid-status",
            TOKEN,
            "status",
            Json::Null,
            None,
            CREATED,
            EXPIRES,
        )
        .expect("build");
        assert_eq!(
            env.to_json().to_canonical_string(),
            fixture("requests/valid-status.command.json").to_canonical_string()
        );
    }

    #[test]
    fn inspect_envelope_matches_the_recorded_fixture() {
        let payload = qjson::json_obj! {
            "source_mode" => "auto",
            "note_scope" => "selected_or_all",
            "melody_extraction" => qjson::json_obj!{ "mode" => "auto" },
        };
        let env = RequestEnvelope::build(
            "valid-inspect-selection",
            TOKEN,
            "inspect_selection",
            payload,
            None,
            CREATED,
            EXPIRES,
        )
        .expect("build");
        assert_eq!(
            env.to_json().to_canonical_string(),
            fixture("requests/valid-inspect-selection.command.json").to_canonical_string()
        );
    }

    #[test]
    fn inspect_with_preconditions_matches_the_recorded_fixture() {
        let expected = ExpectedProject {
            project_uuid: Some("00000000-0000-4000-8000-000000000001".into()),
            state_change_count: None,
            source_item_guid: Some("00000002-0002-4002-8002-000000000002".into()),
            source_take_guid: Some("00000003-0003-4003-8003-000000000003".into()),
            midi_hash: Some("fnv1a64:fac2c019eba29541".into()),
            tempo_map_hash: Some("fnv1a64:387bdc9bc9f1446a".into()),
            snapshot_hash: Some("fnv1a64:3e7bf035037bcf4f".into()),
        };
        let env = RequestEnvelope::build(
            "valid-inspect-with-preconditions",
            TOKEN,
            "inspect_selection",
            qjson::json_obj! { "note_scope" => "all" },
            Some(&expected),
            CREATED,
            EXPIRES,
        )
        .expect("build");
        assert_eq!(
            env.to_json().to_canonical_string(),
            fixture("requests/valid-inspect-with-preconditions.command.json").to_canonical_string()
        );
    }

    #[test]
    fn transaction_envelopes_match_the_recorded_fixtures() {
        for (stem, command) in [
            ("valid-commit", "commit_candidate"),
            ("valid-discard", "discard_candidate"),
            ("valid-undo", "undo_last_generation"),
        ] {
            let env = RequestEnvelope::build(
                stem,
                TOKEN,
                command,
                qjson::json_obj! { "transaction_id" => "tx-0001" },
                None,
                CREATED,
                EXPIRES,
            )
            .expect("build");
            assert_eq!(
                env.to_json().to_canonical_string(),
                fixture(&format!("requests/{stem}.command.json")).to_canonical_string(),
                "{stem}"
            );
        }
    }

    #[test]
    fn build_rejects_a_path_escaping_request_id() {
        let err = RequestEnvelope::build(
            "../../etc/passwd",
            TOKEN,
            "ping",
            Json::Null,
            None,
            CREATED,
            EXPIRES,
        )
        .expect_err("escape");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
    }

    #[test]
    fn build_rejects_an_unknown_command_locally() {
        let err = RequestEnvelope::build(
            "abc",
            TOKEN,
            "execute_lua",
            Json::Null,
            None,
            CREATED,
            EXPIRES,
        )
        .expect_err("unknown");
        assert_eq!(err.code, codes::UNKNOWN_COMMAND);
        assert!(err.details.get("allowed").and_then(Json::as_arr).is_some());
    }

    #[test]
    fn build_rejects_a_non_object_payload() {
        let err = RequestEnvelope::build(
            "abc",
            TOKEN,
            "ping",
            Json::Arr(vec![Json::Int(1)]),
            None,
            CREATED,
            EXPIRES,
        )
        .expect_err("array payload");
        assert_eq!(err.code, codes::MALFORMED_REQUEST);
    }

    #[test]
    fn build_rejects_an_empty_instance_token() {
        let err = RequestEnvelope::build("abc", "", "ping", Json::Null, None, CREATED, EXPIRES)
            .expect_err("empty token");
        assert_eq!(err.code, codes::INVALID_INSTANCE_TOKEN);
    }

    #[test]
    fn a_null_payload_becomes_an_empty_object() {
        let env = RequestEnvelope::build("abc", TOKEN, "ping", Json::Null, None, CREATED, EXPIRES)
            .expect("build");
        assert_eq!(env.payload, Json::Obj(JsonMap::new()));
    }

    #[test]
    fn an_empty_expected_project_is_omitted() {
        let env = RequestEnvelope::build(
            "abc",
            TOKEN,
            "ping",
            Json::Null,
            Some(&ExpectedProject::default()),
            CREATED,
            EXPIRES,
        )
        .expect("build");
        assert!(env.expected_project.is_none());
        assert!(env.to_json().get("expected_project").is_none());
    }

    #[test]
    fn require_bridge_version_is_opt_in() {
        let env = RequestEnvelope::build("abc", TOKEN, "ping", Json::Null, None, CREATED, EXPIRES)
            .expect("build");
        assert!(env.to_json().get("require_bridge_version").is_none());
        let pinned = env.require_bridge_version("1.0.0");
        assert_eq!(
            pinned
                .to_json()
                .get("require_bridge_version")
                .and_then(Json::as_str),
            Some("1.0.0")
        );
    }

    #[test]
    fn serialize_sorts_keys_and_appends_a_newline() {
        let text = serialize(
            &qjson::json_obj! { "b" => 1, "a" => qjson::json_obj!{ "d" => 2, "c" => 3 } },
        );
        assert_eq!(
            text,
            "{\n  \"a\": {\n    \"c\": 3,\n    \"d\": 2\n  },\n  \"b\": 1\n}\n"
        );
    }

    #[test]
    fn expected_project_round_trips_and_omits_unset_fields() {
        let e = ExpectedProject {
            project_uuid: Some("p".into()),
            snapshot_hash: Some("fnv1a64:0000000000000000".into()),
            ..ExpectedProject::default()
        };
        let j = e.to_json();
        assert_eq!(j.as_obj().expect("obj").len(), 2);
        assert_eq!(ExpectedProject::from_json(&j), e);
        assert!(!e.is_empty());
        assert!(ExpectedProject::default().is_empty());
    }

    #[test]
    fn guid_normalisation_strips_braces_and_lowercases() {
        assert_eq!(ExpectedProject::normalize_guid("{AB12-CD34}"), "ab12-cd34");
        assert_eq!(ExpectedProject::normalize_guid("ab12-cd34"), "ab12-cd34");
    }

    #[test]
    fn parses_the_recorded_success_result() {
        let env =
            ResultEnvelope::parse(&fixture("results/valid-stage.result.json")).expect("parse");
        assert!(env.ok);
        assert!(env.error.is_none());
        assert_eq!(env.command.as_deref(), Some("stage_candidate"));
        assert_eq!(
            env.transaction_id.as_deref(),
            Some("00000000-0000-4000-8000-0000000000d1")
        );
        assert_eq!(env.bridge_version, "1.0.0");
        assert_eq!(env.duration_ms, 12.5);
        let result = env.into_outcome().expect("ok");
        assert_eq!(result.get("note_count").and_then(Json::as_i64), Some(8));
        assert_eq!(result.get("status").and_then(Json::as_str), Some("preview"));
    }

    #[test]
    fn parses_the_recorded_no_midi_source_failure() {
        let env = ResultEnvelope::parse(&fixture("results/error-no-midi-source.result.json"))
            .expect("parse");
        assert!(!env.ok);
        assert!(env.result.is_none());
        let err = env.into_outcome().expect_err("failure");
        assert_eq!(err.code, codes::NO_MIDI_SOURCE);
        assert_eq!(
            err.details
                .get("selected_item_count")
                .and_then(Json::as_i64),
            Some(0)
        );
    }

    #[test]
    fn parses_the_recorded_stale_snapshot_failure_with_details_intact() {
        let env = ResultEnvelope::parse(&fixture("results/error-stale-snapshot.result.json"))
            .expect("parse");
        assert!(!env.ok);
        assert_eq!(
            env.transaction_id.as_deref(),
            Some("00000000-0000-4000-8000-0000000000d1")
        );
        let err = env.into_outcome().expect_err("failure");
        assert_eq!(err.code, codes::STALE_SNAPSHOT);
        assert!(err.is_stale());
        assert_eq!(err.detail_str("expected"), Some("fnv1a64:3e7bf035037bcf4f"));
        assert_eq!(err.detail_str("actual"), Some("fnv1a64:0000000000000000"));
    }

    #[test]
    fn a_result_with_a_foreign_protocol_version_is_rejected() {
        let mut doc = fixture("results/valid-stage.result.json");
        if let Json::Obj(m) = &mut doc {
            m.insert("protocol_version", Json::Str("qlabs-reaper-ipc/999".into()));
        }
        let err = ResultEnvelope::parse(&doc).expect_err("mismatch");
        assert_eq!(err.code, codes::IPC_PROTOCOL_MISMATCH);
    }

    #[test]
    fn a_result_missing_the_ok_discriminator_is_rejected() {
        let mut doc = fixture("results/valid-stage.result.json");
        if let Json::Obj(m) = &mut doc {
            m.remove("ok");
        }
        let err = ResultEnvelope::parse(&doc).expect_err("no ok");
        assert_eq!(err.code, codes::INTERNAL_BRIDGE_ERROR);
    }

    #[test]
    fn a_result_that_is_both_ok_and_errored_is_rejected() {
        let mut doc = fixture("results/valid-stage.result.json");
        if let Json::Obj(m) = &mut doc {
            m.insert("error", IpcError::new(codes::MIDI_CHANGED, "x").to_json());
        }
        let err = ResultEnvelope::parse(&doc).expect_err("both");
        assert_eq!(err.code, codes::INTERNAL_BRIDGE_ERROR);
    }

    #[test]
    fn a_failure_with_no_error_object_is_rejected() {
        let mut doc = fixture("results/error-no-midi-source.result.json");
        if let Json::Obj(m) = &mut doc {
            m.insert("error", Json::Null);
        }
        let err = ResultEnvelope::parse(&doc).expect_err("neither");
        assert_eq!(err.code, codes::INTERNAL_BRIDGE_ERROR);
    }

    #[test]
    fn a_failure_that_also_carries_a_result_is_rejected() {
        let mut doc = fixture("results/error-no-midi-source.result.json");
        if let Json::Obj(m) = &mut doc {
            m.insert("result", qjson::json_obj! { "sneaky" => true });
        }
        let err = ResultEnvelope::parse(&doc).expect_err("both");
        assert_eq!(err.code, codes::INTERNAL_BRIDGE_ERROR);
    }

    #[test]
    fn a_non_object_envelope_is_rejected() {
        let err = ResultEnvelope::parse(&Json::Arr(vec![])).expect_err("array");
        assert_eq!(err.code, codes::INTERNAL_BRIDGE_ERROR);
    }
}
