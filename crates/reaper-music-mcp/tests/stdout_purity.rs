//! stdout carries JSON-RPC and nothing else.
//!
//! This is the one invariant that cannot be checked in process: a banner, a
//! `println!`, a progress bar or a panic message written by some library on the
//! way past would all be invisible to a test that owns its own sink. So this
//! file runs the **real binary**, over real pipes, with logging turned all the
//! way up, and asserts that every byte on stdout is a JSON-RPC message — while
//! stderr is simultaneously required to be noisy, which proves the diagnostics
//! were produced and routed rather than simply absent.

use qjson::{json_obj, Json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

/// The binary under test, built by cargo for this integration test.
const BIN: &str = env!("CARGO_BIN_EXE_qlabs-reaper-music-mcp");

/// Runs the server with `input` on stdin and returns `(stdout, stderr, status)`.
fn serve_with(input: &str, log_level: &str) -> (String, String, i32) {
    let mut child = Command::new(BIN)
        .arg("serve")
        .env("QLABS_MCP_LOG", log_level)
        .env_remove("QLABS_MCP_CONFIG")
        .env_remove("QLABS_MCP_IPC_DIR")
        .env_remove("QLABS_MCP_KNOWLEDGE_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the server binary must be runnable");

    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    // Dropping stdin is the graceful-shutdown signal.
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("the server must exit");
    (
        String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        String::from_utf8(output.stderr).expect("stderr is UTF-8"),
        output.status.code().unwrap_or(-1),
    )
}

fn request(id: i64, method: &str, params: Json) -> String {
    format!(
        "{}\n",
        json_obj! {
            "jsonrpc" => "2.0",
            "id" => id,
            "method" => method,
            "params" => params,
        }
    )
}

fn notification(method: &str, params: Json) -> String {
    format!(
        "{}\n",
        json_obj! { "jsonrpc" => "2.0", "method" => method, "params" => params }
    )
}

/// A session that exercises success, failure, listing and notification paths.
fn busy_session() -> String {
    let mut s = String::new();
    s.push_str(&request(
        1,
        "initialize",
        json_obj! { "protocolVersion" => reaper_music_mcp::MCP_PROTOCOL_VERSION },
    ));
    s.push_str(&notification("notifications/initialized", json_obj! {}));
    s.push_str(&request(2, "tools/list", json_obj! {}));
    s.push_str(&request(3, "resources/list", json_obj! {}));
    s.push_str(&request(4, "resources/templates/list", json_obj! {}));
    s.push_str(&request(5, "prompts/list", json_obj! {}));
    s.push_str(&request(6, "ping", json_obj! {}));
    s.push_str(&request(
        7,
        "resources/read",
        json_obj! { "uri" => "theory://catalog" },
    ));
    s.push_str(&request(
        8,
        "prompts/get",
        json_obj! { "name" => "harmonize-selected-melody" },
    ));
    s.push_str(&request(
        9,
        "tools/call",
        json_obj! { "name" => "reaper.status", "arguments" => json_obj! {} },
    ));
    s.push_str(&request(
        10,
        "tools/call",
        json_obj! {
            "name" => "theory.search",
            "arguments" => json_obj! { "query" => "tritone substitution" },
        },
    ));
    // A tool that needs the bridge, with no bridge: must be a clean error.
    s.push_str(&request(
        11,
        "tools/call",
        json_obj! { "name" => "reaper.inspect_selection", "arguments" => json_obj! {} },
    ));
    // Invalid arguments.
    s.push_str(&request(
        12,
        "tools/call",
        json_obj! {
            "name" => "harmony.generate_candidates",
            "arguments" => json_obj! { "snapshot_id" => "abc", "candidate_count" => 99 },
        },
    ));
    // A prohibited tool name.
    s.push_str(&request(
        13,
        "tools/call",
        json_obj! { "name" => "execute_lua", "arguments" => json_obj! { "code" => "os.exit()" } },
    ));
    // Unknown method, unknown resource, unknown prompt.
    s.push_str(&request(14, "reaper/eval", json_obj! {}));
    s.push_str(&request(
        15,
        "resources/read",
        json_obj! { "uri" => "analysis://../../etc/passwd" },
    ));
    s.push_str(&request(16, "prompts/get", json_obj! { "name" => "nope" }));
    // Garbage that is not JSON at all.
    s.push_str("this is not json\n");
    // Blank lines.
    s.push_str("\n\n");
    // A cancellation for a request that never existed.
    s.push_str(&notification(
        "notifications/cancelled",
        json_obj! { "requestId" => 999 },
    ));
    s
}

#[test]
fn stdout_contains_no_non_mcp_text() {
    let (stdout, stderr, status) = serve_with(&busy_session(), "trace");
    assert_eq!(status, 0, "the server must shut down gracefully at EOF");

    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(!lines.is_empty(), "the server produced no output at all");

    for line in &lines {
        let value =
            Json::parse(line).unwrap_or_else(|e| panic!("stdout line is not JSON ({e}):\n{line}"));
        assert_eq!(
            value.str_field("jsonrpc"),
            Ok("2.0"),
            "every stdout line must be a JSON-RPC message:\n{line}"
        );
        let is_response = value.get("id").is_some()
            && (value.get("result").is_some() || value.get("error").is_some());
        let is_notification = value.get("method").is_some() && value.get("id").is_none();
        assert!(
            is_response || is_notification,
            "stdout line is neither a response nor a notification:\n{line}"
        );
    }

    // Nothing that looks like a log line, a banner or a backtrace.
    for marker in [
        "qlabs-reaper-music-mcp:",
        "[warn]",
        "[info]",
        "[debug]",
        "[trace]",
        "[error]",
        "PANIC",
        "panicked at",
        "stack backtrace",
        "note: run with",
        "warning:",
        "Compiling",
    ] {
        assert!(
            !stdout.contains(marker),
            "stdout contains {marker:?}:\n{stdout}"
        );
    }

    // The diagnostics were produced — they just went to stderr.
    assert!(
        stderr.contains("qlabs-reaper-music-mcp:"),
        "stderr should carry the diagnostics:\n{stderr}"
    );
}

#[test]
fn stdout_stays_pure_with_logging_off() {
    let (stdout, stderr, status) = serve_with(&busy_session(), "off");
    assert_eq!(status, 0);
    assert!(
        stderr.is_empty(),
        "logging off must silence stderr: {stderr}"
    );
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        assert_eq!(
            Json::parse(line).map(|v| v.str_field("jsonrpc") == Ok("2.0")),
            Ok(true)
        );
    }
}

#[test]
fn every_request_gets_exactly_one_response_and_notifications_get_none() {
    let (stdout, _, _) = serve_with(&busy_session(), "off");
    let mut answered: Vec<i64> = Vec::new();
    for line in stdout.lines().filter(|l| !l.trim().is_empty()) {
        let v = Json::parse(line).unwrap();
        if let Some(Json::Int(id)) = v.get("id") {
            answered.push(*id);
        }
    }
    // Ids 1..=16 were requests; the parse error for the garbage line answers
    // with a null id, which is not counted here.
    let mut expected: Vec<i64> = (1..=16).collect();
    answered.sort_unstable();
    expected.sort_unstable();
    assert_eq!(answered, expected, "{stdout}");
}

#[test]
fn a_parse_error_is_answered_on_stdout_as_a_json_rpc_error() {
    let (stdout, _, status) = serve_with("{not json at all\n", "off");
    assert_eq!(status, 0);
    let line = stdout
        .lines()
        .find(|l| !l.trim().is_empty())
        .expect("a reply");
    let v = Json::parse(line).expect("the reply is JSON");
    assert_eq!(v.get("id"), Some(&Json::Null));
    assert_eq!(v.get("error").unwrap().i64_field("code"), Ok(-32700));
}

#[test]
fn an_empty_stream_shuts_down_cleanly_and_says_nothing() {
    let (stdout, _, status) = serve_with("", "off");
    assert_eq!(status, 0);
    assert!(stdout.is_empty(), "an empty session must produce no output");
}

#[test]
fn no_tcp_or_http_listener_is_opened() {
    // The server has no networking code at all; this asserts the observable
    // consequence, which is that a session completes with only stdio.
    let (stdout, _, status) = serve_with(&busy_session(), "off");
    assert_eq!(status, 0);
    assert!(!stdout.contains("listening"));
    assert!(!stdout.contains("127.0.0.1"));
    assert!(!stdout.contains("http://"));
}

// ---------------------------------------------------------------------------
// The panic hook
// ---------------------------------------------------------------------------

/// Environment variable that turns the probe test below into a panicking child.
const PANIC_PROBE: &str = "QLABS_MCP_PANIC_PROBE";

/// Panics on purpose when run as a child, so the parent test can inspect where
/// the message went. Harmless when run normally.
#[test]
fn panic_hook_probe() {
    if std::env::var(PANIC_PROBE).is_err() {
        // Running normally: assert only that installing the hook is safe and
        // that a panic is still catchable, i.e. the hook does not abort.
        reaper_music_mcp::log::install_panic_hook();
        let caught = std::panic::catch_unwind(|| panic!("caught by the test, not by the hook"));
        assert!(caught.is_err());
        // Restore the default hook so later tests report normally.
        let _ = std::panic::take_hook();
        return;
    }
    reaper_music_mcp::log::install_panic_hook();
    panic!("qlabs-panic-probe-marker");
}

#[test]
fn the_panic_hook_writes_to_stderr_and_never_to_stdout() {
    let exe = std::env::current_exe().expect("test binary path");
    let output = Command::new(exe)
        .args(["--exact", "panic_hook_probe", "--nocapture"])
        .env(PANIC_PROBE, "1")
        .env("QLABS_MCP_LOG", "trace")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("re-running this test binary as a child must work");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    assert!(
        stderr.contains("PANIC at") && stderr.contains("qlabs-panic-probe-marker"),
        "the hook must report the panic on stderr:\n{stderr}"
    );
    assert!(
        !stdout.contains("PANIC at"),
        "the panic message must never reach stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("stack backtrace"),
        "no backtrace may reach stdout:\n{stdout}"
    );
}

// ---------------------------------------------------------------------------
// The REAPER-free CLI commands
// ---------------------------------------------------------------------------

fn cli(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(BIN)
        .args(args)
        .env("QLABS_MCP_LOG", "off")
        .env_remove("QLABS_MCP_CONFIG")
        .env_remove("QLABS_MCP_IPC_DIR")
        .env_remove("QLABS_MCP_KNOWLEDGE_DIR")
        .output()
        .expect("the binary runs");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

fn fixture(name: &str) -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .join("fixtures")
        .join(name)
        .display()
        .to_string()
}

#[test]
fn every_reaper_free_command_works_with_no_bridge() {
    for args in [
        vec!["doctor"],
        vec!["doctor", "--json"],
        vec!["validate-knowledge"],
        vec!["validate-knowledge", "--json"],
        vec!["list-profiles"],
        vec!["list-profiles", "--json"],
        vec!["print-mcp-config"],
        vec!["version"],
        vec!["version", "--json"],
    ] {
        let (stdout, stderr, code) = cli(&args);
        assert_eq!(code, 0, "{args:?} failed: {stderr}");
        assert!(!stdout.trim().is_empty(), "{args:?} produced nothing");
    }
}

#[test]
fn the_fixture_commands_exercise_the_real_engine_with_no_bridge() {
    let melody = fixture("melodies/eight_bar_c_major.json");

    let (stdout, stderr, code) = cli(&["analyze-fixture", &melody, "--json"]);
    assert_eq!(code, 0, "{stderr}");
    let analysis = Json::parse(&stdout).expect("analyze-fixture emits JSON");
    let body = analysis.get("analysis").expect("analysis");
    assert!(
        body.get("key")
            .unwrap()
            .arr_field("candidates")
            .unwrap()
            .len()
            > 1,
        "a real analysis ranks several keys"
    );
    assert!(!body.arr_field("phrases").unwrap().is_empty());
    assert!(!body.arr_field("nct_hypotheses").unwrap().is_empty());

    let (stdout, stderr, code) = cli(&[
        "generate-fixture",
        &melody,
        "--profile",
        "jazz_standard",
        "--count",
        "3",
        "--seed",
        "12345",
        "--json",
    ]);
    assert_eq!(code, 0, "{stderr}");
    let generation = Json::parse(&stdout).expect("generate-fixture emits JSON");
    let candidates = generation
        .get("generation")
        .unwrap()
        .arr_field("candidates")
        .unwrap()
        .to_vec();
    assert_eq!(candidates.len(), 3);
    for c in &candidates {
        assert!(!c.arr_field("chords").unwrap().is_empty());
        assert!(!c.arr_field("rule_ids").unwrap().is_empty());
        assert!(!c.str_field("explanation").unwrap().is_empty());
    }
    let strategies: Vec<&str> = candidates
        .iter()
        .map(|c| c.str_field("strategy").unwrap())
        .collect();
    let mut unique = strategies.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 3, "candidates must differ: {strategies:?}");
}

#[test]
fn the_fixture_commands_are_byte_reproducible() {
    let melody = fixture("melodies/eight_bar_c_major.json");
    let (a, _, _) = cli(&[
        "generate-fixture",
        &melody,
        "--count",
        "2",
        "--seed",
        "5",
        "--json",
    ]);
    let (b, _, _) = cli(&[
        "generate-fixture",
        &melody,
        "--count",
        "2",
        "--seed",
        "5",
        "--json",
    ]);
    assert_eq!(a, b, "the same fixture, profile and seed must be identical");
}

#[test]
fn doctor_json_reports_every_required_check() {
    let (stdout, _, code) = cli(&["doctor", "--json"]);
    assert_eq!(code, 0);
    let report = Json::parse(&stdout).expect("doctor --json emits JSON");
    assert_eq!(report.get("ok"), Some(&Json::Bool(true)));
    let ids: Vec<&str> = report
        .arr_field("checks")
        .unwrap()
        .iter()
        .map(|c| c.str_field("id").unwrap())
        .collect();
    for required in [
        "server_version",
        "configuration",
        "instance_token",
        "knowledge_valid",
        "knowledge_hash",
        "bridge_heartbeat",
        "bridge_version",
        "reaper_version",
        "tool_schemas",
        "tool_surface",
    ] {
        assert!(ids.contains(&required), "doctor is missing {required}");
    }
}

#[test]
fn the_serve_loop_answers_in_request_order() {
    let mut input = String::new();
    input.push_str(&request(
        1,
        "initialize",
        json_obj! { "protocolVersion" => reaper_music_mcp::MCP_PROTOCOL_VERSION },
    ));
    for id in 2..=6 {
        input.push_str(&request(id, "ping", json_obj! {}));
    }
    let (stdout, _, _) = serve_with(&input, "off");
    let ids: Vec<i64> = BufReader::new(stdout.as_bytes())
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| match Json::parse(&l).ok()?.get("id") {
            Some(Json::Int(i)) => Some(*i),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec![1, 2, 3, 4, 5, 6]);
}
