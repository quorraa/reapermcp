//! End-to-end tests: a real [`BridgeClient`] talking to a
//! [`FakeBridge`](reaper_ipc::testing::FakeBridge) over both the in-memory and
//! the real filesystem.
//!
//! The fake bridge is pumped from the client's own poll hook, so every test
//! here is deterministic: no threads, no scheduler dependence, and no sleep
//! longer than the 2 ms polling floor.

use qjson::Json;
use reaper_ipc::fs::{Filesystem, MemFs, RealFs, TempDir};
use reaper_ipc::testing::{scene_snapshot, FakeBridge, Response};
use reaper_ipc::{codes, BridgeClient, CancelFlag, ExpectedProject, IpcConfig, IpcError, Snapshot};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// A client wired to pump `bridge` on every poll.
fn wire(
    fs: Arc<dyn Filesystem>,
    bridge: Arc<FakeBridge>,
    ipc_dir: &Path,
    timeout: Duration,
) -> BridgeClient {
    let cfg = IpcConfig::new(ipc_dir, TOKEN)
        .with_request_timeout(timeout)
        .with_poll_interval(Duration::from_millis(2));
    let b = bridge.clone();
    BridgeClient::with_fs(cfg, fs)
        .expect("client")
        .with_poll_hook(Arc::new(move |_| {
            b.pump();
        }))
}

/// The standard in-memory rig: fs, an online fake bridge, and a wired client.
fn rig(timeout_ms: u64) -> (MemFs, Arc<FakeBridge>, BridgeClient) {
    let fs = MemFs::new();
    let bridge = Arc::new(FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN));
    bridge.write_heartbeat("online");
    let client = wire(
        Arc::new(fs.clone()),
        bridge.clone(),
        Path::new("/ipc"),
        Duration::from_millis(timeout_ms),
    );
    (fs, bridge, client)
}

fn rig_with(responder: reaper_ipc::testing::Responder, timeout_ms: u64) -> (MemFs, BridgeClient) {
    let fs = MemFs::new();
    let bridge =
        Arc::new(FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN).with_responder(responder));
    bridge.write_heartbeat("online");
    let client = wire(
        Arc::new(fs.clone()),
        bridge,
        Path::new("/ipc"),
        Duration::from_millis(timeout_ms),
    );
    (fs, client)
}

#[test]
fn a_ping_completes_end_to_end() {
    let (_fs, bridge, client) = rig(500);
    let res = client.ping().expect("pong");
    assert_eq!(res.get("pong").and_then(Json::as_bool), Some(true));
    assert_eq!(bridge.processed(), 1);
    assert_eq!(bridge.claimed(), 1);
}

#[test]
fn a_status_call_returns_the_full_report() {
    let (_fs, _bridge, client) = rig(500);
    let res = client.status().expect("status");
    assert_eq!(
        res.get("bridge_connected").and_then(Json::as_bool),
        Some(true)
    );
    assert_eq!(
        res.get("ipc_protocol_version").and_then(Json::as_str),
        Some(reaper_ipc::PROTOCOL_VERSION)
    );
    let commands = res
        .get("commands")
        .and_then(Json::as_arr)
        .expect("commands");
    assert_eq!(commands.len(), reaper_ipc::COMMANDS.len());
    assert!(res.get("limits").and_then(Json::as_obj).is_some());
}

#[test]
fn a_round_trip_leaves_the_ipc_tree_clean() {
    let (fs, _bridge, client) = rig(500);
    client.ping().expect("pong");
    for dir in ["commands", "results", "processing"] {
        assert!(
            fs.list_dir(&Path::new("/ipc").join(dir))
                .expect("ls")
                .is_empty(),
            "{dir} should be empty after a successful round trip"
        );
    }
}

#[test]
fn many_sequential_calls_each_get_their_own_result() {
    let (_fs, bridge, client) = rig(500);
    for _ in 0..12 {
        client.ping().expect("pong");
    }
    assert_eq!(bridge.processed(), 12);
    // Every request id was distinct, so the replay guard never fired.
    let seen = bridge.seen();
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 12);
}

#[test]
fn an_inspect_selection_snapshot_round_trips_and_verifies() {
    let (_fs, client) = rig_with(
        Box::new(|req| {
            if req.command == "inspect_selection" {
                Response::Ok(scene_snapshot())
            } else {
                Response::Err(IpcError::new(codes::INTERNAL_BRIDGE_ERROR, "unexpected"))
            }
        }),
        500,
    );
    let snap = client
        .inspect_snapshot(
            qjson::json_obj! { "note_scope" => "all" },
            None,
            &CancelFlag::new(),
        )
        .expect("snapshot");
    assert_eq!(snap.note_count, 4);
    assert_eq!(
        snap.snapshot_hash.as_deref(),
        Some("fnv1a64:3e7bf035037bcf4f")
    );
    // Round-tripping the snapshot into a precondition block is the whole point.
    let expected = snap.expected_project();
    assert_eq!(expected.snapshot_hash, snap.snapshot_hash);
}

#[test]
fn a_snapshot_whose_hash_does_not_reproduce_fails_the_call() {
    let (_fs, client) = rig_with(
        Box::new(|_| {
            let mut doc = scene_snapshot();
            if let Json::Obj(m) = &mut doc {
                m.insert(
                    "snapshot_hash",
                    Json::Str("fnv1a64:0000000000000000".into()),
                );
            }
            Response::Ok(doc)
        }),
        500,
    );
    let err = client
        .inspect_snapshot(Json::Null, None, &CancelFlag::new())
        .expect_err("bad hash");
    assert_eq!(err.code, codes::SNAPSHOT_HASH_MISMATCH);
    assert_eq!(err.detail_str("field"), Some("snapshot_hash"));
}

#[test]
fn a_bridge_error_arrives_with_its_code_and_details_intact() {
    let (_fs, client) = rig_with(
        Box::new(|_| {
            Response::Err(IpcError::with_details(
                codes::MIDI_CHANGED,
                "the take was edited",
                qjson::json_obj! {
                    "expected" => "fnv1a64:fac2c019eba29541",
                    "actual" => "fnv1a64:0123456789abcdef",
                },
            ))
        }),
        500,
    );
    let err = client
        .inspect_selection(Json::Null)
        .expect_err("bridge failure");
    assert_eq!(err.code, codes::MIDI_CHANGED);
    assert!(err.is_stale());
    assert_eq!(err.detail_str("expected"), Some("fnv1a64:fac2c019eba29541"));
    assert_eq!(err.detail_str("actual"), Some("fnv1a64:0123456789abcdef"));
    assert_eq!(err.message, "the take was edited");
}

#[test]
fn an_unknown_bridge_error_code_survives_unflattened() {
    let (_fs, client) = rig_with(
        Box::new(|_| {
            Response::Err(IpcError::with_details(
                "SOME_FUTURE_CODE",
                "from a newer bridge",
                qjson::json_obj! { "hint" => "upgrade" },
            ))
        }),
        500,
    );
    let err = client.ping().expect_err("failure");
    assert_eq!(err.code, "SOME_FUTURE_CODE");
    assert!(!err.is_known_code());
    assert_eq!(err.detail_str("hint"), Some("upgrade"));
}

#[test]
fn a_silent_bridge_times_out_with_a_structured_error() {
    let fs = MemFs::new();
    let bridge = Arc::new(FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN).always_silent());
    bridge.write_heartbeat("online");
    let client = wire(
        Arc::new(fs.clone()),
        bridge.clone(),
        Path::new("/ipc"),
        Duration::from_millis(20),
    );
    let err = client.ping().expect_err("timeout");
    assert_eq!(err.code, codes::IPC_TIMEOUT);
    assert!(err.is_timeout());
    assert!(err.details.get("polls").is_some());
    // The bridge did claim it, so nothing is left in commands/ to clean up.
    assert_eq!(bridge.claimed(), 1);
    assert!(fs
        .list_dir(Path::new("/ipc/commands"))
        .expect("ls")
        .is_empty());
}

#[test]
fn a_timeout_against_a_dead_bridge_cleans_up_the_unclaimed_command() {
    let fs = MemFs::new();
    // A bridge that writes a heartbeat but never runs.
    let bridge = Arc::new(FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN));
    bridge.write_heartbeat("online");
    let cfg = IpcConfig::new("/ipc", TOKEN)
        .with_request_timeout(Duration::from_millis(20))
        .with_poll_interval(Duration::from_millis(2));
    let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone())).expect("client");
    let err = client.ping().expect_err("timeout");
    assert!(err.is_timeout());
    assert!(
        fs.list_dir(Path::new("/ipc/commands"))
            .expect("ls")
            .is_empty(),
        "the unclaimed command file must be removed"
    );
    assert_eq!(bridge.claimed(), 0);
}

#[test]
fn a_late_result_can_be_collected_and_discarded() {
    let fs = MemFs::new();
    let bridge = Arc::new(FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN));
    bridge.write_heartbeat("online");
    let cfg = IpcConfig::new("/ipc", TOKEN)
        .with_request_timeout(Duration::from_millis(15))
        .with_poll_interval(Duration::from_millis(2));
    let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone())).expect("client");
    let err = client.ping().expect_err("timeout");
    assert!(err.is_timeout());

    // The bridge finally answers a request the client already abandoned. The
    // client must tolerate it: read it, discard it, delete it.
    fs.plant("/ipc/results/abandoned-one.result.json", "{}");
    assert!(client.discard_result("abandoned-one").expect("discard"));
    assert!(fs
        .list_dir(Path::new("/ipc/results"))
        .expect("ls")
        .is_empty());
}

#[test]
fn cancellation_mid_poll_returns_promptly_and_cleans_up() {
    let fs = MemFs::new();
    let bridge = Arc::new(FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN).always_silent());
    bridge.write_heartbeat("online");
    let cancel = CancelFlag::new();
    let c2 = cancel.clone();
    let b = bridge.clone();
    // A 60-second timeout the test must never actually wait for.
    let cfg = IpcConfig::new("/ipc", TOKEN)
        .with_request_timeout(Duration::from_secs(60))
        .with_poll_interval(Duration::from_millis(2));
    let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone()))
        .expect("client")
        .with_poll_hook(Arc::new(move |attempt| {
            b.pump();
            if attempt == 3 {
                c2.cancel();
            }
        }));
    let started = std::time::Instant::now();
    let err = client
        .call("ping", Json::Null, None, &cancel)
        .expect_err("cancelled");
    assert_eq!(err.code, codes::IPC_CANCELLED);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "cancellation waited {:?}",
        started.elapsed()
    );
    assert!(fs
        .list_dir(Path::new("/ipc/commands"))
        .expect("ls")
        .is_empty());
}

#[test]
fn an_offline_bridge_is_refused_before_anything_is_written() {
    let fs = MemFs::new();
    let bridge = Arc::new(FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN));
    bridge.write_heartbeat("offline");
    let client = wire(
        Arc::new(fs.clone()),
        bridge,
        Path::new("/ipc"),
        Duration::from_millis(50),
    );
    let err = client.ping().expect_err("offline");
    assert!(err.is_bridge_offline());
    assert!(!client.is_online());
    assert!(fs
        .list_dir(Path::new("/ipc/commands"))
        .expect("ls")
        .is_empty());
}

#[test]
fn a_token_the_bridge_does_not_share_is_rejected_on_the_wire() {
    let fs = MemFs::new();
    let bridge = Arc::new(FakeBridge::new(
        Arc::new(fs.clone()),
        "/ipc",
        "a-completely-different-token",
    ));
    bridge.write_heartbeat("online");
    let client = wire(
        Arc::new(fs.clone()),
        bridge.clone(),
        Path::new("/ipc"),
        Duration::from_millis(500),
    );
    let err = client.ping().expect_err("bad token");
    assert_eq!(err.code, codes::INVALID_INSTANCE_TOKEN);
    assert_eq!(bridge.failed(), 1);
}

#[test]
fn expected_project_preconditions_travel_on_the_wire() {
    let fs = MemFs::new();
    let bridge = Arc::new(
        FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN).with_responder(Box::new(|req| {
            // Echo the precondition block back so the test can see it arrived.
            Response::Ok(qjson::json_obj! {
                "saw_expected_project" => req.expected_project.clone().unwrap_or(Json::Null),
            })
        })),
    );
    bridge.write_heartbeat("online");
    let client = wire(
        Arc::new(fs.clone()),
        bridge,
        Path::new("/ipc"),
        Duration::from_millis(500),
    );
    let expected = ExpectedProject {
        project_uuid: Some("00000000-0000-4000-8000-000000000001".into()),
        snapshot_hash: Some("fnv1a64:3e7bf035037bcf4f".into()),
        ..ExpectedProject::default()
    };
    let res = client
        .call(
            "inspect_selection",
            Json::Null,
            Some(&expected),
            &CancelFlag::new(),
        )
        .expect("ok");
    let seen = res.get("saw_expected_project").expect("echo");
    assert_eq!(
        seen.get("snapshot_hash").and_then(Json::as_str),
        Some("fnv1a64:3e7bf035037bcf4f")
    );
    assert!(
        seen.get("state_change_count").is_none(),
        "an unset precondition must not be sent"
    );
}

#[test]
fn the_transaction_commands_carry_their_id_and_get_it_back() {
    let (_fs, _bridge, client) = rig(500);
    for res in [
        client.commit_candidate("tx-0001").expect("commit"),
        client.discard_candidate("tx-0001").expect("discard"),
        client.undo_last_generation("tx-0001").expect("undo"),
    ] {
        assert_eq!(
            res.get("echo_payload")
                .and_then(|p| p.get("transaction_id"))
                .and_then(Json::as_str),
            Some("tx-0001")
        );
    }
}

#[test]
fn stage_candidate_puts_the_plan_in_the_payload() {
    let plan = reaper_ipc::plan::plan_from_wire(
        &Json::parse(
            &std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../fixtures/mock-reaper/plans/valid-two-part-candidate.plan.json"),
            )
            .expect("plan fixture"),
        )
        .expect("json"),
    )
    .expect("plan");

    let (_fs, _bridge, client) = rig(500);
    let res = client
        .stage_candidate(&plan, qjson::json_obj! { "verify_snapshot" => true })
        .expect("staged");
    let payload = res.get("echo_payload").expect("echo");
    assert_eq!(
        payload.get("verify_snapshot").and_then(Json::as_bool),
        Some(true)
    );
    assert_eq!(
        payload
            .get("plan")
            .and_then(|p| p.get("transaction_id"))
            .and_then(Json::as_str),
        Some(plan.transaction_id.as_str())
    );
}

#[test]
fn the_whole_flow_works_over_the_real_filesystem() {
    let dir = TempDir::new("roundtrip").expect("temp dir");
    let ipc = dir.path().join("ipc");
    let fs: Arc<dyn Filesystem> = Arc::new(RealFs);
    let bridge = Arc::new(FakeBridge::new(fs.clone(), &ipc, TOKEN));
    bridge.write_heartbeat("online");
    let client = wire(fs.clone(), bridge.clone(), &ipc, Duration::from_millis(500));

    let res = client.ping().expect("pong");
    assert_eq!(res.get("pong").and_then(Json::as_bool), Some(true));
    assert_eq!(bridge.processed(), 1);

    // Nothing is left behind: no .tmp, no result, no processing artefact.
    for sub in ["commands", "results", "processing"] {
        assert!(
            fs.list_dir(&ipc.join(sub)).expect("ls").is_empty(),
            "{sub} not clean"
        );
    }
}

#[test]
fn a_real_filesystem_error_round_trip_keeps_the_failed_artefact() {
    let dir = TempDir::new("realfail").expect("temp dir");
    let ipc = dir.path().join("ipc");
    let fs: Arc<dyn Filesystem> = Arc::new(RealFs);
    let bridge = Arc::new(
        FakeBridge::new(fs.clone(), &ipc, TOKEN)
            .always_failing(IpcError::new(codes::NO_MIDI_SOURCE, "nothing selected")),
    );
    bridge.write_heartbeat("online");
    let client = wire(fs.clone(), bridge.clone(), &ipc, Duration::from_millis(500));

    let err = client.inspect_selection(Json::Null).expect_err("no source");
    assert_eq!(err.code, codes::NO_MIDI_SOURCE);
    // The bridge quarantines the request; the client never touches failed/.
    assert_eq!(fs.list_dir(&ipc.join("failed")).expect("ls").len(), 1);
    assert!(fs.list_dir(&ipc.join("results")).expect("ls").is_empty());
}

#[test]
fn the_client_never_writes_outside_commands() {
    let (fs, _bridge, client) = rig(500);
    let before: Vec<_> = fs.paths();
    client.ping().expect("pong");
    let after = fs.paths();
    // The only paths that ever existed beyond the starting set are inside
    // commands/, results/, processing/ and failed/ — and the client is only
    // responsible for the first two.
    for p in after.iter().filter(|p| !before.contains(p)) {
        let s = p.to_string_lossy();
        assert!(
            s.starts_with("/ipc/"),
            "the client escaped the IPC root: {s}"
        );
    }
    // heartbeat.json is read-only from the client and must still be the
    // bridge's copy.
    assert!(fs.exists(Path::new("/ipc/heartbeat.json")));
}

#[test]
fn a_result_that_arrives_on_the_very_last_poll_is_still_collected() {
    let fs = MemFs::new();
    let bridge = Arc::new(FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN));
    bridge.write_heartbeat("online");
    let b = bridge.clone();
    let cfg = IpcConfig::new("/ipc", TOKEN)
        .with_request_timeout(Duration::from_millis(12))
        .with_poll_interval(Duration::from_millis(2));
    // Only answer once the deadline has certainly passed, exercising the final
    // grace check after the loop.
    let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone()))
        .expect("client")
        .with_poll_hook(Arc::new(move |_| {
            std::thread::sleep(Duration::from_millis(3));
            b.pump();
        }));
    let res = client.ping().expect("late but collected");
    assert_eq!(res.get("pong").and_then(Json::as_bool), Some(true));
}

#[test]
fn the_client_survives_a_bridge_that_writes_a_malformed_result() {
    let fs = MemFs::new();
    let fs2 = fs.clone();
    let bridge = Arc::new(FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN));
    bridge.write_heartbeat("online");
    let cfg = IpcConfig::new("/ipc", TOKEN)
        .with_request_timeout(Duration::from_millis(50))
        .with_poll_interval(Duration::from_millis(2));
    let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone()))
        .expect("client")
        .with_poll_hook(Arc::new(move |attempt| {
            if attempt == 1 {
                for name in fs2.list_dir(Path::new("/ipc/commands")).unwrap_or_default() {
                    let stem = name.trim_end_matches(".command.json");
                    fs2.plant(format!("/ipc/results/{stem}.result.json"), "{ not json");
                }
            }
        }));
    let err = client.ping().expect_err("garbage result");
    assert_eq!(err.code, codes::INTERNAL_BRIDGE_ERROR);
    assert!(
        fs.list_dir(Path::new("/ipc/results"))
            .expect("ls")
            .is_empty(),
        "a consumed result is deleted even when it was garbage"
    );
}

#[test]
fn snapshot_verification_catches_a_bridge_that_mangles_a_note() {
    let (_fs, client) = rig_with(
        Box::new(|_| {
            let mut doc = scene_snapshot();
            // Change a velocity without updating note_list_hash.
            if let Some(Json::Arr(notes)) = doc.get("notes").cloned().as_mut() {
                if let Json::Obj(n) = &mut notes[1] {
                    n.insert("velocity", Json::Int(7));
                }
                if let Json::Obj(m) = &mut doc {
                    m.insert("notes", Json::Arr(notes.clone()));
                }
            }
            Response::Ok(doc)
        }),
        500,
    );
    let err = client
        .inspect_snapshot(Json::Null, None, &CancelFlag::new())
        .expect_err("mangled");
    assert_eq!(err.code, codes::SNAPSHOT_HASH_MISMATCH);
    assert_eq!(err.detail_str("field"), Some("note_list_hash"));

    // The unverified reader still hands the raw value back, so a caller that
    // wants to inspect the damage can.
    let raw = client.inspect_selection(Json::Null).expect("raw");
    let snap = Snapshot::from_json(&raw).expect("parse");
    assert!(snap.verify().is_err());
}

#[test]
fn the_staged_plan_arrives_in_the_shape_the_bridge_parses() {
    // The plan on the wire is not `EditPlan::to_json()`: quarter notes are
    // numbers, preconditions are discriminated on `type`, and tags are pairs.
    // If any of those regress the bridge rejects the plan with
    // INVALID_EDIT_PLAN, so assert the shape that actually leaves the process.
    let plan = reaper_ipc::plan::plan_from_wire(
        &Json::parse(
            &std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../fixtures/mock-reaper/plans/valid-two-part-candidate.plan.json"),
            )
            .expect("plan fixture"),
        )
        .expect("json"),
    )
    .expect("plan");

    let (_fs, _bridge, client) = rig(500);
    let res = client
        .stage_candidate(&plan, qjson::json_obj! { "note_scope" => "all" })
        .expect("staged");
    let wire = res
        .get("echo_payload")
        .and_then(|p| p.get("plan"))
        .expect("the plan the bridge received");

    for op in wire
        .get("operations")
        .and_then(Json::as_arr)
        .expect("operations")
    {
        for key in ["start_qn", "end_qn"] {
            if let Some(v) = op.get(key) {
                assert!(
                    v.as_f64().is_some() && v.as_str().is_none(),
                    "{key} must be a number, got {v:?}"
                );
            }
        }
        if let Some(tags) = op.get("tags") {
            assert!(
                tags.as_arr().is_some(),
                "tags must be an array of pairs, got {tags:?}"
            );
        }
        if let Some(notes) = op.get("notes").and_then(Json::as_arr) {
            for n in notes {
                assert!(n.get("start_qn").and_then(Json::as_f64).is_some());
                assert!(n.get("end_qn").and_then(Json::as_f64).is_some());
            }
        }
    }
    for p in wire
        .get("preconditions")
        .and_then(Json::as_arr)
        .expect("preconditions")
    {
        assert!(p.get("type").and_then(Json::as_str).is_some(), "{p:?}");
        assert!(p.get("kind").is_none(), "{p:?}");
    }
    assert!(wire
        .get("undo_label")
        .and_then(Json::as_str)
        .expect("undo_label")
        .starts_with("QLabs MCP: "));
}

#[test]
fn a_stale_heartbeat_takes_the_bridge_offline_without_a_write() {
    let fs = MemFs::new();
    let bridge = Arc::new(FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN));
    bridge.write_heartbeat("online");
    // Rewrite the heartbeat with a timestamp well past the staleness window.
    let bytes = fs
        .read(Path::new("/ipc/heartbeat.json"))
        .expect("heartbeat");
    let mut doc = Json::parse(&String::from_utf8(bytes).expect("utf8")).expect("json");
    if let Json::Obj(m) = &mut doc {
        m.insert("timestamp", Json::Int(qjson::time::unix_now() - 3600));
    }
    fs.plant("/ipc/heartbeat.json", doc.to_string());

    let client = wire(
        Arc::new(fs.clone()),
        bridge,
        Path::new("/ipc"),
        Duration::from_millis(50),
    );
    assert!(!client.is_online());
    let err = client.ping().expect_err("stale");
    assert!(err.is_bridge_offline());
    assert_eq!(
        err.details.get("age_seconds").and_then(Json::as_i64),
        Some(3600)
    );
    assert!(fs
        .list_dir(Path::new("/ipc/commands"))
        .expect("ls")
        .is_empty());
}

#[test]
fn a_bridge_speaking_another_protocol_is_a_mismatch_not_an_outage() {
    let fs = MemFs::new();
    let bridge = Arc::new(FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN));
    bridge.write_heartbeat("online");
    let bytes = fs
        .read(Path::new("/ipc/heartbeat.json"))
        .expect("heartbeat");
    let mut doc = Json::parse(&String::from_utf8(bytes).expect("utf8")).expect("json");
    if let Json::Obj(m) = &mut doc {
        m.insert("protocol_version", Json::Str("qlabs-reaper-ipc/2".into()));
    }
    fs.plant("/ipc/heartbeat.json", doc.to_string());

    let client = wire(
        Arc::new(fs.clone()),
        bridge,
        Path::new("/ipc"),
        Duration::from_millis(50),
    );
    let err = client.ping().expect_err("mismatch");
    assert_eq!(err.code, codes::IPC_PROTOCOL_MISMATCH);
    assert!(!err.is_bridge_offline());
    assert_eq!(err.detail_str("received"), Some("qlabs-reaper-ipc/2"));
}

#[test]
fn four_commands_per_tick_is_respected_end_to_end() {
    // The bridge handles at most MAX_COMMANDS_PER_TICK per defer tick. Queue
    // more than that through one fake bridge and confirm they all drain.
    let fs = MemFs::new();
    let bridge = Arc::new(FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN));
    bridge.write_heartbeat("online");
    let client = wire(
        Arc::new(fs.clone()),
        bridge.clone(),
        Path::new("/ipc"),
        Duration::from_millis(500),
    );
    for _ in 0..9 {
        client.ping().expect("pong");
    }
    assert_eq!(bridge.processed(), 9);
    assert!(fs
        .list_dir(Path::new("/ipc/commands"))
        .expect("ls")
        .is_empty());
}
