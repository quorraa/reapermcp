//! Golden tests against `fixtures/mock-reaper/`.
//!
//! Every recorded request envelope is parsed and pushed through the same
//! validation the bridge runs; every recorded result envelope is parsed by the
//! client's reader; every recorded hash vector is reproduced. These fixtures
//! are regenerated from the live Lua implementation, so a failure here means
//! the two sides have drifted apart.

use qjson::Json;
use reaper_ipc::envelope::{ExpectedProject, RequestEnvelope, ResultEnvelope};
use reaper_ipc::protocol::{validate_envelope, ValidationContext};
use reaper_ipc::{codes, limits, Snapshot};
use std::path::{Path, PathBuf};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";
/// `2026-07-26T18:51:20Z`, the "now" the fixture README pins.
const NOW: i64 = 1_785_091_880;
/// `2026-07-26T18:51:19Z` / `2026-07-26T18:51:49Z`.
const CREATED: i64 = 1_785_091_879;
const EXPIRES: i64 = 1_785_091_909;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mock-reaper")
}

fn read(rel: &str) -> String {
    let p = root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn json(rel: &str) -> Json {
    Json::parse(&read(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn list(rel: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root().join(rel))
        .expect("fixture directory")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn ctx<'a>(stem: &'a str) -> ValidationContext<'a> {
    ValidationContext {
        instance_token: TOKEN,
        now: NOW,
        request_id_hint: Some(stem),
        seen: &[],
        raw_size: None,
        bridge_version: limits::BRIDGE_VERSION,
    }
}

/// The outcome each recorded request fixture must produce, per the README.
fn expected_outcome(stem: &str) -> Option<&'static str> {
    match stem {
        "valid-ping"
        | "valid-status"
        | "valid-inspect-selection"
        | "valid-inspect-with-preconditions"
        | "valid-commit"
        | "valid-discard"
        | "valid-undo" => None,
        "invalid-protocol-version" => Some(codes::IPC_PROTOCOL_MISMATCH),
        "invalid-token" => Some(codes::INVALID_INSTANCE_TOKEN),
        "invalid-expired" => Some(codes::EXPIRED_REQUEST),
        "invalid-unknown-command" => Some(codes::UNKNOWN_COMMAND),
        "invalid-request-id" => Some(codes::MALFORMED_REQUEST),
        "invalid-payload-is-array" => Some(codes::MALFORMED_REQUEST),
        other => panic!("unclassified request fixture {other:?}: add it to this table"),
    }
}

#[test]
fn the_request_fixture_set_is_complete() {
    let names = list("requests");
    assert_eq!(
        names.len(),
        14,
        "expected the 14 recorded request fixtures, found {names:?}"
    );
    for n in &names {
        assert!(n.ends_with(".command.json"), "{n}");
    }
}

#[test]
fn every_request_fixture_produces_its_documented_outcome() {
    for name in list("requests") {
        let stem = name.trim_end_matches(".command.json").to_string();
        if stem == "invalid-truncated" {
            // Documented as "not valid JSON at all".
            assert!(
                Json::parse(&read(&format!("requests/{name}"))).is_err(),
                "{stem} should not parse"
            );
            continue;
        }
        let doc = json(&format!("requests/{name}"));
        // The escaping-id fixture cannot be given a stem hint: its own id is
        // what must be refused, at step 7, before any path exists.
        let hint = if stem == "invalid-request-id" {
            None
        } else {
            Some(stem.as_str())
        };
        let mut c = ctx(&stem);
        c.request_id_hint = hint;
        let got = validate_envelope(&doc, &c);
        match expected_outcome(&stem) {
            None => {
                got.unwrap_or_else(|e| panic!("{stem} should be accepted, got {e}"));
            }
            Some(code) => {
                let err = got
                    .err()
                    .unwrap_or_else(|| panic!("{stem} should be rejected"));
                assert_eq!(err.code, code, "{stem}");
            }
        }
    }
}

#[test]
fn every_valid_request_fixture_is_reproduced_byte_for_byte_by_the_builder() {
    // Each entry is (stem, command, payload, expected_project).
    let cases: Vec<(&str, &str, Json, Option<ExpectedProject>)> = vec![
        ("valid-ping", "ping", qjson::json_obj! {}, None),
        ("valid-status", "status", qjson::json_obj! {}, None),
        (
            "valid-inspect-selection",
            "inspect_selection",
            qjson::json_obj! {
                "source_mode" => "auto",
                "note_scope" => "selected_or_all",
                "melody_extraction" => qjson::json_obj!{ "mode" => "auto" },
            },
            None,
        ),
        (
            "valid-inspect-with-preconditions",
            "inspect_selection",
            qjson::json_obj! { "note_scope" => "all" },
            Some(ExpectedProject {
                project_uuid: Some("00000000-0000-4000-8000-000000000001".into()),
                state_change_count: None,
                source_item_guid: Some("00000002-0002-4002-8002-000000000002".into()),
                source_take_guid: Some("00000003-0003-4003-8003-000000000003".into()),
                midi_hash: Some("fnv1a64:fac2c019eba29541".into()),
                tempo_map_hash: Some("fnv1a64:387bdc9bc9f1446a".into()),
                snapshot_hash: Some("fnv1a64:3e7bf035037bcf4f".into()),
            }),
        ),
        (
            "valid-commit",
            "commit_candidate",
            qjson::json_obj! { "transaction_id" => "tx-0001" },
            None,
        ),
        (
            "valid-discard",
            "discard_candidate",
            qjson::json_obj! { "transaction_id" => "tx-0001" },
            None,
        ),
        (
            "valid-undo",
            "undo_last_generation",
            qjson::json_obj! { "transaction_id" => "tx-0001" },
            None,
        ),
    ];

    for (stem, command, payload, expected) in cases {
        let env = RequestEnvelope::build(
            stem,
            TOKEN,
            command,
            payload,
            expected.as_ref(),
            CREATED,
            EXPIRES,
        )
        .unwrap_or_else(|e| panic!("{stem}: {e}"));
        let recorded = read(&format!("requests/{stem}.command.json"));
        let built = String::from_utf8(env.to_bytes()).expect("utf8");
        assert_eq!(built, recorded, "{stem} does not match the recorded bytes");
    }
}

#[test]
fn every_result_fixture_is_read_by_the_client_reader() {
    let names = list("results");
    assert_eq!(
        names.len(),
        3,
        "expected 3 recorded results, found {names:?}"
    );
    for name in names {
        let doc = json(&format!("results/{name}"));
        let env = ResultEnvelope::parse(&doc).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(env.protocol_version, limits::PROTOCOL_VERSION);
        assert_eq!(env.bridge_version, limits::BRIDGE_VERSION);
        // `ok` is the discriminator: exactly one of result/error is non-null.
        assert_eq!(env.ok, env.error.is_none(), "{name}");
        assert_eq!(env.ok, env.result.is_some(), "{name}");
        let outcome = env.into_outcome();
        assert_eq!(
            outcome.is_ok(),
            doc.get("ok").and_then(Json::as_bool) == Some(true)
        );
    }
}

#[test]
fn the_recorded_failures_keep_their_code_and_details() {
    let e = ResultEnvelope::parse(&json("results/error-no-midi-source.result.json"))
        .expect("parse")
        .into_outcome()
        .expect_err("failure");
    assert_eq!(e.code, codes::NO_MIDI_SOURCE);
    assert_eq!(
        e.details.get("selected_item_count").and_then(Json::as_i64),
        Some(0)
    );

    let e = ResultEnvelope::parse(&json("results/error-stale-snapshot.result.json"))
        .expect("parse")
        .into_outcome()
        .expect_err("failure");
    assert_eq!(e.code, codes::STALE_SNAPSHOT);
    assert!(e.is_stale());
    assert_eq!(e.detail_str("expected"), Some("fnv1a64:3e7bf035037bcf4f"));
}

#[test]
fn every_error_code_the_bridge_can_emit_is_known_to_this_build() {
    let doc = json("bridge/error-codes.json");
    for v in doc.arr_field("codes").expect("codes") {
        let code = v.as_str().expect("string");
        assert!(
            codes::is_known(code),
            "{code} is in the bridge's vocabulary but not in this build's"
        );
    }
}

#[test]
fn the_recorded_snapshot_parses_and_every_hash_reproduces() {
    let snap =
        Snapshot::from_json(&json("snapshots/four-note-melody.snapshot.json")).expect("parse");
    let checked = snap.verify().expect("hashes reproduce");
    assert_eq!(checked.len(), 6, "all six hashes should be verifiable");
    assert_eq!(snap.note_count, 4);
    assert_eq!(snap.notes.len(), 4);
}

#[test]
fn the_recorded_snapshot_hash_matches_the_recorded_precondition_block() {
    let snap =
        Snapshot::from_json(&json("snapshots/four-note-melody.snapshot.json")).expect("parse");
    let recorded = json("requests/valid-inspect-with-preconditions.command.json");
    let expected =
        ExpectedProject::from_json(recorded.get("expected_project").expect("expected_project"));
    let derived = snap.expected_project();
    assert_eq!(derived.project_uuid, expected.project_uuid);
    assert_eq!(derived.snapshot_hash, expected.snapshot_hash);
    assert_eq!(derived.midi_hash, expected.midi_hash);
    assert_eq!(derived.tempo_map_hash, expected.tempo_map_hash);
    assert_eq!(derived.source_item_guid, expected.source_item_guid);
    assert_eq!(derived.source_take_guid, expected.source_take_guid);
}

#[test]
fn every_recorded_hash_vector_reproduces() {
    let doc = json("hashes/vectors.json");
    let vectors = doc.arr_field("vectors").expect("vectors");
    assert!(vectors.len() >= 11);
    for v in vectors {
        let name = v.str_field("name").expect("name");
        let cs = v.str_field("canonical_string").expect("canonical_string");
        let want = v.str_field("hash").expect("hash");
        assert_eq!(
            reaper_ipc::hash::qlabs_hash(cs.as_bytes()),
            want,
            "vector {name}"
        );
    }
}

#[test]
fn the_recorded_bridge_config_loads() {
    let cfg = reaper_ipc::IpcConfig::from_config_json(
        &json("bridge/config.json"),
        Path::new("/scripts/QLabs"),
    )
    .expect("load");
    cfg.validate().expect("valid");
    assert_eq!(cfg.instance_token, TOKEN);
}

#[test]
fn the_recorded_heartbeat_reports_online_at_its_own_timestamp() {
    let doc = json("bridge/heartbeat.json");
    let ts = doc
        .get("timestamp")
        .and_then(Json::as_i64)
        .expect("timestamp");
    let hb = reaper_ipc::Heartbeat::parse(&doc, ts).expect("online");
    assert!(hb.commands_match_this_build());
    assert!(reaper_ipc::Heartbeat::parse(&doc, ts + 3600).is_err());
}

#[test]
fn the_recorded_bridge_lock_is_readable_and_never_written() {
    // The client reads bridge.lock for diagnostics only. Assert the shape it
    // needs is there; nothing in this crate ever writes it.
    let doc = json("bridge/bridge.lock");
    assert_eq!(
        doc.get("protocol_version").and_then(Json::as_str),
        Some(limits::PROTOCOL_VERSION)
    );
    assert!(doc.get("pid_token").and_then(Json::as_str).is_some());
    assert!(doc.get("heartbeat_at").and_then(Json::as_i64).is_some());
}

#[test]
fn the_valid_plan_fixture_passes_local_preflight() {
    let plan = reaper_ipc::plan::plan_from_wire(&json("plans/valid-two-part-candidate.plan.json"))
        .expect("plan parses");
    reaper_ipc::client::preflight_plan(&plan).expect("preflight");
    assert!(plan.undo_label.starts_with(limits::UNDO_LABEL_PREFIX));
    assert_eq!(plan.operations.len(), 9);
    assert_eq!(plan.expected_outputs.len(), 5);
}

#[test]
fn the_rejected_plan_fixtures_are_refused_locally_or_are_the_bridges_to_refuse() {
    let index = json("plans/invalid-index.json");
    // Which rejections the client can make on its own, without a project.
    let locally_catchable = [
        "invalid-unowned-undo-label.plan.json",
        "invalid-knowledge-version.plan.json",
    ];

    for entry in index.arr_field("plans").expect("plans") {
        let file = entry.str_field("file").expect("file");
        let want = entry.str_field("expected_error_code").expect("code");
        let doc = json(&format!("plans/{file}"));
        let parsed = reaper_ipc::plan::plan_from_wire(&doc);

        if locally_catchable.contains(&file) {
            let plan = parsed.unwrap_or_else(|e| panic!("{file} should still parse: {e}"));
            let err = reaper_ipc::client::preflight_plan(&plan)
                .expect_err(&format!("{file} must be refused locally"));
            assert_eq!(err.code, want, "{file}");
        } else {
            // Everything else needs a live project to evaluate: either the
            // domain parser already rejects it, or preflight lets it through
            // and the bridge decides. Both are correct; what must not happen is
            // a panic or a silent acceptance of a plan the parser rejected.
            if let Ok(plan) = parsed {
                let _ = reaper_ipc::client::preflight_plan(&plan);
            }
            assert!(codes::is_known(want), "{file} expects unknown code {want}");
        }
    }
}
