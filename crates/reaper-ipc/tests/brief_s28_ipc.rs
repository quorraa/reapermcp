//! The brief §28 "IPC" checklist, one test per required behaviour.
//!
//! Each test is named after the line in the checklist it discharges, so a
//! reviewer can read the list and the test names side by side:
//!
//! 1. partial `.tmp` files are ignored
//! 2. the command file is claimed atomically
//! 3. invalid token rejected
//! 4. expired request rejected
//! 5. oversized request rejected
//! 6. unknown command rejected
//! 7. duplicate request id rejected
//! 8. result files are atomic
//! 9. timeout produces a structured error
//! 10. stale snapshot rejected
//!
//! Where a behaviour has both a client-side and a bridge-side aspect, both are
//! covered.

use qjson::Json;
use reaper_ipc::envelope::serialize;
use reaper_ipc::fs::{Filesystem, MemFs, RealFs, TempDir};
use reaper_ipc::testing::{scene_snapshot, FakeBridge, Response};
use reaper_ipc::{codes, limits, BridgeClient, CancelFlag, IpcConfig, IpcError};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn envelope(stem: &str, command: &str) -> Json {
    qjson::json_obj! {
        "protocol_version" => limits::PROTOCOL_VERSION,
        "request_id" => stem,
        "instance_token" => TOKEN,
        "created_at" => qjson::time::now_iso8601(),
        "expires_at" => qjson::time::iso8601_from_unix(qjson::time::unix_now() + 30),
        "command" => command,
        "payload" => qjson::json_obj!{},
    }
}

fn plant_command(fs: &MemFs, stem: &str, doc: &Json) {
    fs.plant(format!("/ipc/commands/{stem}.command.json"), serialize(doc));
}

fn error_code(fs: &MemFs, stem: &str) -> String {
    let bytes = fs
        .read(Path::new(&format!("/ipc/results/{stem}.result.json")))
        .expect("result file");
    let doc = Json::parse(&String::from_utf8(bytes).expect("utf8")).expect("json");
    doc.get("error")
        .and_then(|e| e.get("code"))
        .and_then(Json::as_str)
        .unwrap_or("<none>")
        .to_string()
}

fn bridge_on(fs: &MemFs) -> Arc<FakeBridge> {
    let b = Arc::new(FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN));
    b.write_heartbeat("online");
    b
}

fn wired(fs: &MemFs, bridge: Arc<FakeBridge>, timeout_ms: u64) -> BridgeClient {
    let cfg = IpcConfig::new("/ipc", TOKEN)
        .with_request_timeout(Duration::from_millis(timeout_ms))
        .with_poll_interval(Duration::from_millis(2));
    BridgeClient::with_fs(cfg, Arc::new(fs.clone()))
        .expect("client")
        .with_poll_hook(Arc::new(move |_| {
            bridge.pump();
        }))
}

// ---- 1. partial `.tmp` files are ignored ------------------------------------

#[test]
fn s28_partial_command_tmp_files_are_never_claimed_or_read() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    fs.plant(
        "/ipc/commands/half-written.tmp",
        "{\"protocol_version\": \"qlabs-reaper-i",
    );
    assert!(bridge.list_commands().is_empty());
    assert_eq!(bridge.pump(), 0);
    assert_eq!(bridge.claimed(), 0);
    assert!(
        fs.exists(Path::new("/ipc/commands/half-written.tmp")),
        "a foreign .tmp must be left alone, not consumed"
    );
    assert!(fs
        .list_dir(Path::new("/ipc/results"))
        .expect("ls")
        .is_empty());
}

#[test]
fn s28_a_partial_result_tmp_is_never_read_by_the_client() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    let fs2 = fs.clone();
    let cfg = IpcConfig::new("/ipc", TOKEN)
        .with_request_timeout(Duration::from_millis(16))
        .with_poll_interval(Duration::from_millis(2));
    let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone()))
        .expect("client")
        .with_poll_hook(Arc::new(move |attempt| {
            if attempt == 1 {
                for name in fs2.list_dir(Path::new("/ipc/commands")).unwrap_or_default() {
                    let stem = name.trim_end_matches(".command.json");
                    // A half-written result in the bridge's staging name.
                    fs2.plant(
                        format!("/ipc/results/{stem}.result.tmp"),
                        "{\"protocol_version\": \"qla",
                    );
                }
            }
        }));
    let err = client.ping().expect_err("nothing complete to read");
    assert_eq!(err.code, codes::IPC_TIMEOUT);
    assert_eq!(
        fs.list_dir(Path::new("/ipc/results")).expect("ls").len(),
        1,
        "the bridge-owned .result.tmp must survive untouched"
    );
    let _ = bridge;
}

#[test]
fn s28_the_client_leaves_no_tmp_behind_after_publishing() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    let client = wired(&fs, bridge, 200);
    client.ping().expect("pong");
    for p in fs.paths() {
        let s = p.to_string_lossy();
        assert!(!s.ends_with(".tmp"), "stray tmp file {s}");
    }
}

// ---- 2. the command file is claimed atomically -------------------------------

#[test]
fn s28_a_command_file_is_claimed_by_exactly_one_renamer() {
    let fs = MemFs::new();
    let a = bridge_on(&fs);
    // A second bridge instance sharing the same directory.
    let b = Arc::new(FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN));
    plant_command(&fs, "contested", &envelope("contested", "ping"));

    assert_eq!(a.pump(), 1, "the first pump wins the rename");
    assert_eq!(b.pump(), 0, "the loser finds nothing to claim");
    assert_eq!(a.claimed() + b.claimed(), 1, "exactly one claim");
    assert_eq!(
        fs.list_dir(Path::new("/ipc/results")).expect("ls"),
        vec!["contested.result.json"],
        "exactly one result is produced"
    );
}

#[test]
fn s28_the_claim_moves_the_file_out_of_commands() {
    let fs = MemFs::new();
    let bridge = Arc::new(FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN).always_silent());
    plant_command(&fs, "claimed", &envelope("claimed", "ping"));
    bridge.pump();
    assert!(
        fs.list_dir(Path::new("/ipc/commands"))
            .expect("ls")
            .is_empty(),
        "the claimed file must not remain visible in commands/"
    );
    assert_eq!(
        fs.list_dir(Path::new("/ipc/processing")).expect("ls"),
        vec!["claimed.processing.json"]
    );
}

#[test]
fn s28_a_claimed_command_is_not_deleted_by_client_timeout_cleanup() {
    let fs = MemFs::new();
    let bridge = Arc::new(FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN).always_silent());
    bridge.write_heartbeat("online");
    let client = wired(&fs, bridge.clone(), 20);
    let err = client.ping().expect_err("silent bridge");
    assert!(err.is_timeout());
    assert_eq!(bridge.claimed(), 1);
    assert_eq!(
        fs.list_dir(Path::new("/ipc/processing")).expect("ls").len(),
        1,
        "cleanup must never reach into processing/"
    );
}

// ---- 3. invalid token rejected ----------------------------------------------

#[test]
fn s28_an_invalid_instance_token_is_rejected() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    let mut doc = envelope("bad-token", "ping");
    if let Json::Obj(m) = &mut doc {
        m.insert(
            "instance_token",
            Json::Str("not-the-installation-token".into()),
        );
    }
    plant_command(&fs, "bad-token", &doc);
    bridge.pump();
    assert_eq!(error_code(&fs, "bad-token"), codes::INVALID_INSTANCE_TOKEN);
}

#[test]
fn s28_a_missing_instance_token_is_rejected() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    let mut doc = envelope("no-token", "ping");
    if let Json::Obj(m) = &mut doc {
        m.remove("instance_token");
    }
    plant_command(&fs, "no-token", &doc);
    bridge.pump();
    assert_eq!(error_code(&fs, "no-token"), codes::INVALID_INSTANCE_TOKEN);
}

#[test]
fn s28_a_token_of_the_right_bytes_but_wrong_length_is_rejected() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    let mut doc = envelope("short-token", "ping");
    if let Json::Obj(m) = &mut doc {
        m.insert("instance_token", Json::Str(TOKEN[..16].to_string()));
    }
    plant_command(&fs, "short-token", &doc);
    bridge.pump();
    assert_eq!(
        error_code(&fs, "short-token"),
        codes::INVALID_INSTANCE_TOKEN
    );
}

// ---- 4. expired request rejected --------------------------------------------

#[test]
fn s28_an_expired_request_is_rejected() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    let mut doc = envelope("expired", "ping");
    if let Json::Obj(m) = &mut doc {
        m.insert("expires_at", Json::Str("2020-01-01T00:00:00Z".into()));
    }
    plant_command(&fs, "expired", &doc);
    bridge.pump();
    assert_eq!(error_code(&fs, "expired"), codes::EXPIRED_REQUEST);
}

#[test]
fn s28_expiry_tolerates_exactly_the_documented_clock_skew() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    let expires = qjson::time::unix_now();
    let mut doc = envelope("skewed", "ping");
    if let Json::Obj(m) = &mut doc {
        m.insert(
            "expires_at",
            Json::Str(qjson::time::iso8601_from_unix(expires)),
        );
    }

    bridge.set_now(expires + limits::CLOCK_SKEW_SECONDS);
    plant_command(&fs, "skewed", &doc);
    bridge.pump();
    assert_eq!(
        error_code(&fs, "skewed"),
        "<none>",
        "a request within the skew window must be accepted"
    );

    fs.remove_file(Path::new("/ipc/results/skewed.result.json"))
        .expect("collect");
    bridge.set_now(expires + limits::CLOCK_SKEW_SECONDS + 1);
    plant_command(&fs, "skewed2", &envelope("skewed2", "ping"));
    let mut doc2 = envelope("skewed2", "ping");
    if let Json::Obj(m) = &mut doc2 {
        m.insert(
            "expires_at",
            Json::Str(qjson::time::iso8601_from_unix(expires)),
        );
    }
    plant_command(&fs, "skewed2", &doc2);
    bridge.pump();
    assert_eq!(error_code(&fs, "skewed2"), codes::EXPIRED_REQUEST);
}

// ---- 5. oversized request rejected -------------------------------------------

#[test]
fn s28_an_oversized_request_is_rejected_by_the_bridge() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    let mut doc = envelope("huge", "ping");
    if let Json::Obj(m) = &mut doc {
        m.insert(
            "payload",
            qjson::json_obj! { "blob" => "x".repeat(limits::MAX_REQUEST_BYTES + 64) },
        );
    }
    plant_command(&fs, "huge", &doc);
    bridge.pump();
    assert_eq!(error_code(&fs, "huge"), codes::PAYLOAD_TOO_LARGE);
}

#[test]
fn s28_an_oversized_request_never_reaches_the_filesystem_from_the_client() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    let mut cfg = IpcConfig::new("/ipc", TOKEN).with_request_timeout(Duration::from_millis(20));
    cfg.max_request_bytes = 512;
    let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone())).expect("client");
    let err = client
        .call(
            "ping",
            qjson::json_obj! { "blob" => "x".repeat(8192) },
            None,
            &CancelFlag::new(),
        )
        .expect_err("too large");
    assert_eq!(err.code, codes::PAYLOAD_TOO_LARGE);
    assert!(fs
        .list_dir(Path::new("/ipc/commands"))
        .expect("ls")
        .is_empty());
    assert_eq!(bridge.claimed(), 0);
}

#[test]
fn s28_an_oversized_result_is_refused_after_reading() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    let fs2 = fs.clone();
    let mut cfg = IpcConfig::new("/ipc", TOKEN)
        .with_request_timeout(Duration::from_millis(40))
        .with_poll_interval(Duration::from_millis(2));
    cfg.max_result_bytes = 128;
    let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone()))
        .expect("client")
        .with_poll_hook(Arc::new(move |_| {
            bridge.pump();
            let _ = &fs2;
        }));
    // The default responder's `status` result is far larger than 128 bytes.
    let err = client.status().expect_err("too large");
    assert_eq!(err.code, codes::RESULT_TOO_LARGE);
    assert!(
        fs.list_dir(Path::new("/ipc/results"))
            .expect("ls")
            .is_empty(),
        "an oversized result is still consumed"
    );
}

// ---- 6. unknown command rejected ---------------------------------------------

#[test]
fn s28_an_unknown_command_is_rejected_by_the_bridge_with_the_allowlist() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    let mut doc = envelope("evil", "execute_lua");
    if let Json::Obj(m) = &mut doc {
        m.insert("payload", qjson::json_obj! { "code" => "os.exit()" });
    }
    plant_command(&fs, "evil", &doc);
    bridge.pump();
    assert_eq!(error_code(&fs, "evil"), codes::UNKNOWN_COMMAND);
}

#[test]
fn s28_an_unknown_command_is_refused_locally_before_any_write() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    let client = wired(&fs, bridge.clone(), 20);
    for bad in ["execute_lua", "run_action", "read_file", "PING", ""] {
        let err = client
            .call(bad, Json::Null, None, &CancelFlag::new())
            .expect_err("unknown");
        assert_eq!(err.code, codes::UNKNOWN_COMMAND, "{bad}");
    }
    assert!(fs
        .list_dir(Path::new("/ipc/commands"))
        .expect("ls")
        .is_empty());
    assert_eq!(bridge.claimed(), 0);
}

#[test]
fn s28_the_allowlist_is_exactly_the_seven_documented_commands() {
    assert_eq!(
        limits::COMMANDS,
        [
            "commit_candidate",
            "discard_candidate",
            "inspect_selection",
            "ping",
            "stage_candidate",
            "status",
            "undo_last_generation",
        ]
    );
}

// ---- 7. duplicate request id rejected -----------------------------------------

#[test]
fn s28_a_duplicate_request_id_is_rejected() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    plant_command(&fs, "dup", &envelope("dup", "ping"));
    bridge.pump();
    assert_eq!(error_code(&fs, "dup"), "<none>");
    fs.remove_file(Path::new("/ipc/results/dup.result.json"))
        .expect("collect");

    plant_command(&fs, "dup", &envelope("dup", "ping"));
    bridge.pump();
    assert_eq!(error_code(&fs, "dup"), codes::DUPLICATE_REQUEST);
}

#[test]
fn s28_the_client_never_reuses_a_request_id() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    let client = wired(&fs, bridge.clone(), 200);
    for _ in 0..25 {
        client.ping().expect("pong");
    }
    let mut seen = bridge.seen();
    assert_eq!(seen.len(), 25);
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 25, "every request id must be fresh");
    assert_eq!(bridge.failed(), 0, "no duplicate was ever reported");
}

// ---- 8. result files are atomic ------------------------------------------------

#[test]
fn s28_a_result_appears_complete_or_not_at_all() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    plant_command(&fs, "atomic", &envelope("atomic", "status"));
    // Before the pump, nothing.
    assert!(fs
        .list_dir(Path::new("/ipc/results"))
        .expect("ls")
        .is_empty());
    bridge.pump();
    // After the pump, exactly one complete, parseable file — never a .result.tmp.
    let names = fs.list_dir(Path::new("/ipc/results")).expect("ls");
    assert_eq!(names, vec!["atomic.result.json"]);
    let bytes = fs
        .read(Path::new("/ipc/results/atomic.result.json"))
        .expect("read");
    let doc = Json::parse(&String::from_utf8(bytes).expect("utf8")).expect("complete json");
    assert_eq!(doc.get("ok").and_then(Json::as_bool), Some(true));
}

#[test]
fn s28_result_publishing_uses_rename_on_the_real_filesystem() {
    let dir = TempDir::new("atomicresult").expect("temp dir");
    let ipc = dir.path().join("ipc");
    let fs: Arc<dyn Filesystem> = Arc::new(RealFs);
    let bridge = FakeBridge::new(fs.clone(), &ipc, TOKEN);
    let doc = envelope("atomic", "ping");
    fs.write_sync(
        &ipc.join("commands").join("atomic.command.json"),
        serialize(&doc).as_bytes(),
    )
    .expect("plant");
    bridge.pump();
    let names = fs.list_dir(&ipc.join("results")).expect("ls");
    assert_eq!(names, vec!["atomic.result.json"]);
}

#[test]
fn s28_the_client_consumes_a_result_exactly_once() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    let client = wired(&fs, bridge, 200);
    client.ping().expect("pong");
    assert!(fs
        .list_dir(Path::new("/ipc/results"))
        .expect("ls")
        .is_empty());
}

// ---- 9. timeout produces a structured error --------------------------------------

#[test]
fn s28_a_timeout_produces_a_structured_ipc_timeout() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    let cfg = IpcConfig::new("/ipc", TOKEN)
        .with_request_timeout(Duration::from_millis(20))
        .with_poll_interval(Duration::from_millis(2));
    let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone())).expect("client");
    let err = client.ping().expect_err("timeout");
    assert_eq!(err.code, codes::IPC_TIMEOUT);
    assert!(err.is_timeout());
    assert!(!err.message.is_empty());
    assert_eq!(
        err.details.get("timeout_ms").and_then(Json::as_i64),
        Some(20)
    );
    assert!(err
        .details
        .get("request_id")
        .and_then(Json::as_str)
        .is_some());
    let _ = bridge;
}

#[test]
fn s28_a_timeout_removes_the_unclaimed_command_file() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    let cfg = IpcConfig::new("/ipc", TOKEN)
        .with_request_timeout(Duration::from_millis(20))
        .with_poll_interval(Duration::from_millis(2));
    let client = BridgeClient::with_fs(cfg, Arc::new(fs.clone())).expect("client");
    client.ping().expect_err("timeout");
    assert!(fs
        .list_dir(Path::new("/ipc/commands"))
        .expect("ls")
        .is_empty());
    let _ = bridge;
}

#[test]
fn s28_the_timeout_error_implements_the_standard_error_traits() {
    let e = IpcError::new(codes::IPC_TIMEOUT, "no result");
    let _: &dyn std::error::Error = &e;
    assert_eq!(e.to_string(), "IPC_TIMEOUT: no result");
}

// ---- 10. stale snapshot rejected ---------------------------------------------

#[test]
fn s28_a_stale_snapshot_error_propagates_with_its_details() {
    let fs = MemFs::new();
    let bridge = Arc::new(
        FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN).with_responder(Box::new(|_| {
            Response::Err(IpcError::with_details(
                codes::STALE_SNAPSHOT,
                "the project no longer matches the snapshot this plan was generated from",
                qjson::json_obj! {
                    "expected" => "fnv1a64:3e7bf035037bcf4f",
                    "actual" => "fnv1a64:0000000000000000",
                },
            ))
        })),
    );
    bridge.write_heartbeat("online");
    let client = wired(&fs, bridge, 200);
    let err = client.inspect_selection(Json::Null).expect_err("stale");
    assert_eq!(err.code, codes::STALE_SNAPSHOT);
    assert!(err.is_stale());
    assert_eq!(err.detail_str("expected"), Some("fnv1a64:3e7bf035037bcf4f"));
    assert_eq!(err.detail_str("actual"), Some("fnv1a64:0000000000000000"));
}

#[test]
fn s28_a_snapshot_whose_hashes_do_not_reproduce_is_rejected_locally() {
    let fs = MemFs::new();
    let bridge = Arc::new(
        FakeBridge::new(Arc::new(fs.clone()), "/ipc", TOKEN).with_responder(Box::new(|_| {
            let mut doc = scene_snapshot();
            if let Json::Obj(m) = &mut doc {
                // The project moved but the bridge forgot to rehash.
                m.insert("item_length_qn", Json::Float(16.0));
            }
            Response::Ok(doc)
        })),
    );
    bridge.write_heartbeat("online");
    let client = wired(&fs, bridge, 200);
    let err = client
        .inspect_snapshot(Json::Null, None, &CancelFlag::new())
        .expect_err("does not reproduce");
    assert_eq!(err.code, codes::SNAPSHOT_HASH_MISMATCH);
}

#[test]
fn s28_the_recorded_stale_snapshot_result_is_read_as_a_stale_error() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/mock-reaper/results/error-stale-snapshot.result.json");
    let doc = Json::parse(&std::fs::read_to_string(path).expect("fixture")).expect("json");
    let err = reaper_ipc::ResultEnvelope::parse(&doc)
        .expect("parse")
        .into_outcome()
        .expect_err("stale");
    assert_eq!(err.code, codes::STALE_SNAPSHOT);
    assert!(err.is_stale());
}

// ---- path-escape refusal ------------------------------------------------------

#[test]
fn s28_no_caller_value_can_escape_the_ipc_directory() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    let client = wired(&fs, bridge, 20);
    let escapes = [
        "../../etc/passwd",
        "..",
        "a/../../b",
        "/absolute",
        "a/b",
        "a\\b",
        "C:\\Windows",
        "..sneaky",
        "",
    ];
    for bad in escapes {
        assert!(
            client.discard_result(bad).is_err(),
            "discard_result accepted {bad:?}"
        );
        assert!(
            client.commit_candidate(bad).is_err(),
            "commit_candidate accepted {bad:?}"
        );
        assert!(
            reaper_ipc::protocol::validate_request_id(bad).is_err(),
            "validate_request_id accepted {bad:?}"
        );
        let cfg = IpcConfig::new("/ipc", TOKEN);
        assert!(cfg.command_path(bad).is_err(), "command_path built {bad:?}");
        assert!(cfg.result_path(bad).is_err(), "result_path built {bad:?}");
    }
    // Nothing was created anywhere.
    for p in fs.paths() {
        let s = p.to_string_lossy();
        assert!(s.starts_with("/ipc/"), "escaped path {s}");
    }
}

#[test]
fn s28_a_command_file_with_an_escaping_stem_is_quarantined_not_executed() {
    let fs = MemFs::new();
    let bridge = bridge_on(&fs);
    // A file whose stem is not a legal id. The bridge must not let it dictate
    // the result path, and must not execute it.
    fs.plant(
        "/ipc/commands/..escape.command.json",
        serialize(&envelope("..escape", "ping")),
    );
    bridge.pump();
    assert_eq!(bridge.claimed(), 0);
    assert_eq!(bridge.processed(), 0);
    assert!(
        fs.list_dir(Path::new("/ipc/results"))
            .expect("ls")
            .is_empty(),
        "no result may be written for an unusable stem"
    );
    assert_eq!(bridge.quarantined().len(), 1);
}

// ---- config loading and validation --------------------------------------------

#[test]
fn s28_config_loading_and_validation_are_covered() {
    let dir = TempDir::new("cfg").expect("temp dir");
    let path = dir.path().join("config.json");

    // A valid config.
    std::fs::write(
        &path,
        serialize(&qjson::json_obj! {
            "instance_token" => TOKEN,
            "ipc_dir" => Json::Null,
            "poll_interval_ms" => 50,
            "heartbeat_interval_ms" => 1000,
            "log_level" => "info",
            "console_log" => false,
        }),
    )
    .expect("write");
    let cfg = IpcConfig::from_config_file(&path).expect("load");
    assert_eq!(cfg.instance_token, TOKEN);
    assert_eq!(cfg.ipc_dir, dir.path().join("ipc"));
    cfg.validate().expect("valid");

    // A config with no token: the server must not invent one.
    std::fs::write(&path, "{\"ipc_dir\": null}").expect("write");
    assert_eq!(
        IpcConfig::from_config_file(&path)
            .expect_err("no token")
            .code,
        codes::INVALID_INSTANCE_TOKEN
    );

    // A config that is not JSON.
    std::fs::write(&path, "nope").expect("write");
    assert_eq!(
        IpcConfig::from_config_file(&path)
            .expect_err("bad json")
            .code,
        codes::MALFORMED_REQUEST
    );

    // A config that does not exist.
    assert_eq!(
        IpcConfig::from_config_file(&dir.path().join("missing.json"))
            .expect_err("missing")
            .code,
        codes::IPC_IO_ERROR
    );
}
