//! The brief §28 "MCP protocol" test list, driven through the real dispatcher.
//!
//! Each of the eight required behaviours has at least one test named after it:
//!
//! | §28 requirement | test |
//! |---|---|
//! | Initialize succeeds | `initialize_succeeds` |
//! | Tool listing succeeds | `tool_listing_succeeds` |
//! | Resource listing succeeds | `resource_listing_succeeds` |
//! | Prompt listing succeeds | `prompt_listing_succeeds` |
//! | Invalid arguments produce structured errors | `invalid_arguments_produce_structured_errors` |
//! | Standard output contains no non-MCP text | `stdout_contains_no_non_mcp_text_in_process` (and the out-of-process test in `stdout_purity.rs`) |
//! | Tool output validates against declared schema | `tool_output_validates_against_declared_schema` |
//! | Cancellation does not corrupt state | `cancellation_does_not_corrupt_state` |
//! | Expired candidate IDs are rejected | `expired_candidate_ids_are_rejected` |

use qjson::{json_arr, json_obj, Json};
use reaper_music_mcp::rpc::Writer;
use reaper_music_mcp::server::{CallContext, Server, ServerCore};
use reaper_music_mcp::MCP_PROTOCOL_VERSION;
use std::io::Write;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A `Write` that records everything, shareable across the test.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Sink {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().expect("lock").clone()).expect("utf-8")
    }

    fn lines(&self) -> Vec<String> {
        self.text()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect()
    }
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("lock").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Harness {
    server: Server,
    sink: Sink,
    next_id: i64,
}

impl Harness {
    fn new() -> Harness {
        let sink = Sink::default();
        let server = Server::new(ServerCore::offline(), Writer::new(Box::new(sink.clone())));
        Harness {
            server,
            sink,
            next_id: 1,
        }
    }

    fn started() -> Harness {
        let mut h = Harness::new();
        h.request(
            "initialize",
            json_obj! { "protocolVersion" => MCP_PROTOCOL_VERSION },
        );
        h.notify("notifications/initialized", json_obj! {});
        h
    }

    fn request(&mut self, method: &str, params: Json) -> Json {
        let id = self.next_id;
        self.next_id += 1;
        let message = json_obj! {
            "jsonrpc" => "2.0",
            "id" => id,
            "method" => method,
            "params" => params,
        };
        let response = self
            .server
            .handle(&message)
            .unwrap_or_else(|| panic!("{method} must produce a response"));
        assert_eq!(response.get("id"), Some(&Json::Int(id)));
        assert_eq!(response.str_field("jsonrpc"), Ok("2.0"));
        self.sink_send(&response);
        response
    }

    fn notify(&mut self, method: &str, params: Json) {
        let message = json_obj! {
            "jsonrpc" => "2.0",
            "method" => method,
            "params" => params,
        };
        assert!(
            self.server.handle(&message).is_none(),
            "{method} is a notification and must not be answered"
        );
    }

    /// Mirrors a response into the sink, the way the serve loop does.
    fn sink_send(&self, response: &Json) {
        let mut guard = self.sink.0.lock().expect("lock");
        guard.extend_from_slice(response.to_string().as_bytes());
        guard.push(b'\n');
    }

    fn result(&mut self, method: &str, params: Json) -> Json {
        let r = self.request(method, params);
        r.get("result")
            .unwrap_or_else(|| panic!("{method} failed: {r:?}"))
            .clone()
    }

    fn call_tool(&mut self, name: &str, arguments: Json) -> Json {
        self.result(
            "tools/call",
            json_obj! { "name" => name, "arguments" => arguments },
        )
    }

    fn structured(&mut self, name: &str, arguments: Json) -> (bool, Json) {
        let r = self.call_tool(name, arguments);
        let is_error = r.get("isError") == Some(&Json::Bool(true));
        (
            is_error,
            r.get("structuredContent").expect("structured").clone(),
        )
    }
}

/// The repository's `fixtures/` directory.
fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace root")
        .join("fixtures")
}

/// Seeds a core with a snapshot taken from a repository fixture.
fn seed_snapshot(core: &ServerCore, fixture: &str) -> String {
    let path = fixtures_dir().join(format!("{fixture}.json"));
    let f = music_domain::fixture::Fixture::from_path(&path).expect("fixture loads");
    let record = reaper_music_mcp::fixtures::snapshot_record(&f).expect("snapshot builds");
    core.with_store(|s| s.put_snapshot(record, qjson::time::unix_now()))
}

// ---------------------------------------------------------------------------
// §28: initialize
// ---------------------------------------------------------------------------

#[test]
fn initialize_succeeds() {
    let mut h = Harness::new();
    let result = h.result(
        "initialize",
        json_obj! {
            "protocolVersion" => MCP_PROTOCOL_VERSION,
            "capabilities" => json_obj! {},
            "clientInfo" => json_obj! { "name" => "test", "version" => "0" },
        },
    );
    assert_eq!(
        result.str_field("protocolVersion").unwrap(),
        MCP_PROTOCOL_VERSION
    );
    let caps = result.get("capabilities").expect("capabilities");
    assert_eq!(
        caps.get("tools").unwrap().get("listChanged"),
        Some(&Json::Bool(false))
    );
    assert_eq!(
        caps.get("resources").unwrap().get("listChanged"),
        Some(&Json::Bool(false))
    );
    assert_eq!(
        caps.get("prompts").unwrap().get("listChanged"),
        Some(&Json::Bool(false))
    );
    let info = result.get("serverInfo").expect("serverInfo");
    assert_eq!(
        info.str_field("name").unwrap(),
        reaper_music_mcp::SERVER_NAME
    );
    assert_eq!(
        info.str_field("version").unwrap(),
        reaper_music_mcp::SERVER_VERSION
    );
    assert!(!result.str_field("instructions").unwrap().is_empty());
}

#[test]
fn initialize_with_an_older_protocol_still_succeeds_and_reports_ours() {
    let mut h = Harness::new();
    let result = h.result(
        "initialize",
        json_obj! { "protocolVersion" => "2024-11-05" },
    );
    assert_eq!(
        result.str_field("protocolVersion").unwrap(),
        MCP_PROTOCOL_VERSION
    );
}

#[test]
fn ping_answers_with_an_empty_result() {
    let mut h = Harness::started();
    let result = h.result("ping", json_obj! {});
    assert_eq!(result, Json::Obj(Default::default()));
}

#[test]
fn requests_before_initialize_are_refused() {
    let mut h = Harness::new();
    let r = h.request("tools/list", json_obj! {});
    assert!(r.get("error").is_some());
}

// ---------------------------------------------------------------------------
// §28: listing
// ---------------------------------------------------------------------------

#[test]
fn tool_listing_succeeds() {
    let mut h = Harness::started();
    let tools = h.result("tools/list", json_obj! {});
    let list = tools.arr_field("tools").expect("tools");
    assert_eq!(list.len(), 14);
    let names: Vec<&str> = list.iter().map(|t| t.str_field("name").unwrap()).collect();
    assert_eq!(
        names,
        vec![
            "reaper.status",
            "reaper.inspect_selection",
            "theory.search",
            "music.analyze_selection",
            "harmony.generate_candidates",
            "harmony.reharmonize",
            "voicing.generate",
            "arrangement.generate",
            "loop.audit",
            "candidate.explain",
            "reaper.stage_candidate",
            "reaper.commit_candidate",
            "reaper.discard_candidate",
            "reaper.undo_last_generation",
        ]
    );
    for t in list {
        assert!(!t.str_field("description").unwrap().is_empty());
        let input = t.get("inputSchema").expect("inputSchema");
        let output = t.get("outputSchema").expect("outputSchema");
        assert!(qjson::schema::Schema::compile(input).is_ok());
        assert!(qjson::schema::Schema::compile(output).is_ok());
    }
}

#[test]
fn no_prohibited_tool_is_listed() {
    let mut h = Harness::started();
    let listed = h.result("tools/list", json_obj! {}).to_string();
    for bad in [
        "execute_lua",
        "execute_shell",
        "run_reaper_action",
        "write_arbitrary_midi",
        "delete_track_by_name",
        "edit_project_chunk",
        "read_file",
        "write_file",
    ] {
        assert!(!listed.contains(bad), "{bad} must never be exposed");
    }
}

#[test]
fn calling_a_prohibited_tool_is_a_structured_unknown_tool_error() {
    let mut h = Harness::started();
    for bad in ["execute_lua", "run_reaper_action", "delete_track_by_name"] {
        let (is_error, payload) = h.structured(bad, json_obj! {});
        assert!(is_error, "{bad}");
        assert_eq!(payload.str_field("error_code").unwrap(), "UNKNOWN_TOOL");
    }
}

#[test]
fn resource_listing_succeeds() {
    let mut h = Harness::started();
    let result = h.result("resources/list", json_obj! {});
    let list = result.arr_field("resources").expect("resources");
    assert_eq!(list.len(), 6);
    let uris: Vec<&str> = list.iter().map(|r| r.str_field("uri").unwrap()).collect();
    assert_eq!(
        uris,
        vec![
            "reaper://status",
            "reaper://project/current",
            "reaper://selection/current",
            "theory://catalog",
            "theory://sources",
            "theory://profiles",
        ]
    );
    for r in list {
        assert!(!r.str_field("name").unwrap().is_empty());
        assert_eq!(r.str_field("mimeType").unwrap(), "application/json");
    }
}

#[test]
fn resource_template_listing_succeeds() {
    let mut h = Harness::started();
    let result = h.result("resources/templates/list", json_obj! {});
    let list = result.arr_field("resourceTemplates").expect("templates");
    assert_eq!(list.len(), 7);
    let uris: Vec<&str> = list
        .iter()
        .map(|r| r.str_field("uriTemplate").unwrap())
        .collect();
    assert_eq!(
        uris,
        vec![
            "theory://profiles/{profile_id}",
            "theory://rules/{domain}",
            "analysis://{analysis_id}",
            "candidate://{candidate_id}",
            "candidate://{candidate_id}/trace",
            "editplan://{plan_id}",
            "transaction://{transaction_id}",
        ]
    );
}

#[test]
fn prompt_listing_succeeds() {
    let mut h = Harness::started();
    let result = h.result("prompts/list", json_obj! {});
    let list = result.arr_field("prompts").expect("prompts");
    assert_eq!(list.len(), 9);
    let names: Vec<&str> = list.iter().map(|p| p.str_field("name").unwrap()).collect();
    assert_eq!(
        names,
        vec![
            "analyze-selected-melody",
            "harmonize-selected-melody",
            "reharmonize-selected-region",
            "extend-selected-chords",
            "create-smooth-voicings",
            "arrange-selected-sketch",
            "create-loopable-variants",
            "audit-harmony-and-voice-leading",
            "explain-generated-candidate",
        ]
    );
}

#[test]
fn every_prompt_can_be_fetched() {
    let mut h = Harness::started();
    let list = h.result("prompts/list", json_obj! {});
    for p in list.arr_field("prompts").unwrap().to_vec() {
        let name = p.str_field("name").unwrap().to_string();
        let required: Vec<String> = p
            .arr_field("arguments")
            .unwrap()
            .iter()
            .filter(|a| a.get("required") == Some(&Json::Bool(true)))
            .map(|a| a.str_field("name").unwrap().to_string())
            .collect();
        let mut arguments = qjson::JsonMap::new();
        for r in &required {
            arguments.insert(r.clone(), Json::Str("seeded-id".into()));
        }
        let got = h.result(
            "prompts/get",
            json_obj! { "name" => name.clone(), "arguments" => Json::Obj(arguments) },
        );
        let messages = got.arr_field("messages").expect("messages");
        assert_eq!(messages.len(), 1, "{name}");
        assert_eq!(messages[0].str_field("role").unwrap(), "user");
        let text = messages[0]
            .get("content")
            .unwrap()
            .str_field("text")
            .unwrap();
        assert!(!text.is_empty(), "{name}");
        assert!(!text.contains("{style_clause}"), "{name}");
    }
}

#[test]
fn an_unknown_prompt_is_refused() {
    let mut h = Harness::started();
    let r = h.request("prompts/get", json_obj! { "name" => "does-not-exist" });
    assert!(r.get("error").is_some());
}

#[test]
fn a_resource_read_returns_json_contents() {
    let mut h = Harness::started();
    let result = h.result("resources/read", json_obj! { "uri" => "theory://catalog" });
    let contents = result.arr_field("contents").expect("contents");
    assert_eq!(contents.len(), 1);
    let text = contents[0].str_field("text").unwrap();
    let body = Json::parse(text).expect("valid JSON");
    assert!(body.get("knowledge_version").is_some());
}

#[test]
fn a_resource_uri_can_never_name_a_filesystem_path() {
    let mut h = Harness::started();
    for uri in [
        "analysis://../../etc/passwd",
        "analysis:///etc/passwd",
        "candidate://../../../root/.bashrc",
        "editplan://../../Cargo.toml",
        "transaction://..%2f..%2fetc%2fshadow",
        "file:///etc/passwd",
        "theory://profiles/../../../etc/hosts",
        "theory://rules/../../secrets",
        "reaper://status/../../etc/passwd",
        "analysis://C:\\Windows\\win.ini",
        "analysis://~/.ssh/id_rsa",
        "analysis://%2e%2e/%2e%2e/etc",
    ] {
        let r = h.request("resources/read", json_obj! { "uri" => uri });
        let error = r
            .get("error")
            .unwrap_or_else(|| panic!("{uri} was served!"));
        assert_eq!(
            error.i64_field("code"),
            Ok(reaper_music_mcp::resources::RESOURCE_NOT_FOUND),
            "{uri}"
        );
        let text = r.to_string();
        assert!(
            !text.contains("root:") && !text.contains("ssh-rsa"),
            "{uri} leaked file content"
        );
    }
}

// ---------------------------------------------------------------------------
// §28: invalid arguments
// ---------------------------------------------------------------------------

#[test]
fn invalid_arguments_produce_structured_errors() {
    let mut h = Harness::started();
    let cases: Vec<(&str, Json)> = vec![
        // Out of range.
        (
            "harmony.generate_candidates",
            json_obj! { "snapshot_id" => "abc", "candidate_count" => 9 },
        ),
        (
            "harmony.generate_candidates",
            json_obj! { "snapshot_id" => "abc", "candidate_count" => 0 },
        ),
        (
            "harmony.generate_candidates",
            json_obj! { "snapshot_id" => "abc", "complexity" => 1.5 },
        ),
        (
            "harmony.generate_candidates",
            json_obj! { "snapshot_id" => "abc", "chromaticism" => -0.1 },
        ),
        // Wrong type.
        ("music.analyze_selection", json_obj! { "snapshot_id" => 12 }),
        // Unknown enum member.
        (
            "music.analyze_selection",
            json_obj! { "snapshot_id" => "abc", "style_profile" => "not_a_profile" },
        ),
        (
            "music.analyze_selection",
            json_obj! { "snapshot_id" => "abc", "strictness" => "whatever" },
        ),
        // Unknown argument.
        (
            "music.analyze_selection",
            json_obj! { "snapshot_id" => "abc", "styleProfile" => "jazz_standard" },
        ),
        // Missing required argument.
        ("theory.search", json_obj! {}),
        ("candidate.explain", json_obj! {}),
        ("reaper.commit_candidate", json_obj! {}),
        // Path-shaped identifier.
        (
            "candidate.explain",
            json_obj! { "candidate_id" => "../../etc/passwd" },
        ),
        // Wrong shape for a nested object.
        (
            "reaper.inspect_selection",
            json_obj! { "melody_extraction" => json_arr![1, 2] },
        ),
        (
            "reaper.inspect_selection",
            json_obj! { "melody_extraction" => json_obj! { "mode" => "sideways" } },
        ),
        // Array element out of range.
        (
            "arrangement.generate",
            json_obj! { "candidate_id" => "abc", "density" => 4.0 },
        ),
    ];

    for (tool, args) in cases {
        let (is_error, payload) = h.structured(tool, args.clone());
        assert!(is_error, "{tool} {args:?} should have failed");
        let code = payload.str_field("error_code").unwrap();
        assert_eq!(code, "INVALID_ARGUMENTS", "{tool} {args:?}");
        let violations = payload
            .get("details")
            .unwrap()
            .arr_field("violations")
            .unwrap();
        assert!(!violations.is_empty(), "{tool}");
        for v in violations {
            assert!(v.get("path").is_some());
            assert!(v.get("keyword").is_some());
            assert!(v.get("message").is_some());
        }
    }
}

#[test]
fn an_invalid_argument_never_becomes_a_transport_error() {
    let mut h = Harness::started();
    let r = h.request(
        "tools/call",
        json_obj! {
            "name" => "theory.search",
            "arguments" => json_obj! { "max_results" => 500 },
        },
    );
    assert!(
        r.get("error").is_none(),
        "argument validation must not be a JSON-RPC error"
    );
    assert_eq!(
        r.get("result").unwrap().get("isError"),
        Some(&Json::Bool(true))
    );
}

#[test]
fn every_tool_error_payload_satisfies_the_shared_envelope_schema() {
    let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .unwrap()
        .join("schemas/mcp-tool-output.schema.json");
    let doc = Json::parse(&std::fs::read_to_string(&schema_path).expect("schema file"))
        .expect("schema parses");
    let schema = qjson::schema::Schema::compile(&doc).expect("schema compiles");

    let mut h = Harness::started();
    // A failure payload.
    let (_, failure) = h.structured("candidate.explain", json_obj! { "candidate_id" => "nope" });
    assert!(
        schema.validate(&failure).is_empty(),
        "failure payload: {failure:?}"
    );
    // A success payload.
    let (_, success) = h.structured("reaper.status", json_obj! {});
    assert!(
        schema.validate(&success).is_empty(),
        "success payload: {success:?}"
    );
}

// ---------------------------------------------------------------------------
// §28: stdout purity (in process; the out-of-process test lives in
// stdout_purity.rs and drives the real binary)
// ---------------------------------------------------------------------------

#[test]
fn stdout_contains_no_non_mcp_text_in_process() {
    let mut h = Harness::started();
    h.result("tools/list", json_obj! {});
    h.result("resources/list", json_obj! {});
    h.result("prompts/list", json_obj! {});
    h.call_tool("reaper.status", json_obj! {});
    h.call_tool("theory.search", json_obj! { "query" => "cadence" });
    // Failures too: an error must not put prose on stdout.
    h.call_tool("candidate.explain", json_obj! { "candidate_id" => "nope" });
    h.request("nope/nope", json_obj! {});

    let lines = h.sink.lines();
    assert!(!lines.is_empty());
    for line in &lines {
        let v =
            Json::parse(line).unwrap_or_else(|e| panic!("stdout line is not JSON: {e}\n{line}"));
        assert_eq!(
            v.str_field("jsonrpc"),
            Ok("2.0"),
            "every stdout line must be a JSON-RPC message: {line}"
        );
        assert!(
            v.get("result").is_some() || v.get("error").is_some() || v.get("method").is_some(),
            "not a response or notification: {line}"
        );
    }
}

#[test]
fn progress_notifications_are_valid_protocol_messages() {
    let sink = Sink::default();
    let core = ServerCore::offline();
    let writer = Writer::new(Box::new(sink.clone()));
    let ctx = CallContext {
        progress_token: Some(Json::Str("tok-1".into())),
        writer: Some(writer),
        ..CallContext::detached()
    };
    let snapshot_id = seed_snapshot(&core, "melodies/eight_bar_c_major");
    let result = reaper_music_mcp::tools::call(
        &core,
        "harmony.generate_candidates",
        &json_obj! { "snapshot_id" => snapshot_id, "candidate_count" => 2, "seed" => 11 },
        &ctx,
    );
    assert_eq!(result.get("isError"), Some(&Json::Bool(false)));

    let lines = sink.lines();
    assert!(!lines.is_empty(), "progress must have been emitted");
    for line in &lines {
        let v = Json::parse(line).expect("progress line is JSON");
        assert_eq!(v.str_field("jsonrpc"), Ok("2.0"));
        assert_eq!(v.str_field("method"), Ok("notifications/progress"));
        let params = v.get("params").unwrap();
        assert_eq!(
            params.get("progressToken"),
            Some(&Json::Str("tok-1".into()))
        );
        let p = params.f64_field("progress").unwrap();
        assert!((0.0..=1.0).contains(&p), "{p}");
    }
}

// ---------------------------------------------------------------------------
// §28: output schemas
// ---------------------------------------------------------------------------

#[test]
fn tool_output_validates_against_declared_schema() {
    let core = ServerCore::offline();
    let snapshot_id = seed_snapshot(&core, "melodies/eight_bar_c_major");
    let ctx = CallContext::detached();

    // Every REAPER-free tool, exercised for real and checked against the
    // schema it declares in tools/list.
    let check = |name: &str, body: &Json| {
        let tool = core.tools.get(name).expect(name);
        let violations = tool.output.validate(body);
        assert!(
            violations.is_empty(),
            "{name} violates its own outputSchema: {:?}\n{body:?}",
            violations
                .iter()
                .map(|v| format!("{}{}: {}", v.instance_path, v.keyword, v.message))
                .collect::<Vec<_>>()
        );
    };

    let status = call_ok(&core, "reaper.status", json_obj! {}, &ctx);
    check("reaper.status", &status);

    let search = call_ok(
        &core,
        "theory.search",
        json_obj! { "query" => "voice leading" },
        &ctx,
    );
    check("theory.search", &search);

    let analysis = call_ok(
        &core,
        "music.analyze_selection",
        json_obj! { "snapshot_id" => snapshot_id.clone() },
        &ctx,
    );
    check("music.analyze_selection", &analysis);
    let analysis_id = analysis.str_field("analysis_id").unwrap().to_string();

    let generation = call_ok(
        &core,
        "harmony.generate_candidates",
        json_obj! {
            "snapshot_id" => snapshot_id.clone(),
            "analysis_id" => analysis_id.clone(),
            "candidate_count" => 3,
            "seed" => 77,
        },
        &ctx,
    );
    check("harmony.generate_candidates", &generation);
    let candidate_id = generation.arr_field("candidates").unwrap()[0]
        .str_field("candidate_id")
        .unwrap()
        .to_string();

    let reharm = call_ok(
        &core,
        "harmony.reharmonize",
        json_obj! { "candidate_id" => candidate_id.clone(), "candidate_count" => 2 },
        &ctx,
    );
    check("harmony.reharmonize", &reharm);

    let voicing = call_ok(
        &core,
        "voicing.generate",
        json_obj! { "candidate_id" => candidate_id.clone() },
        &ctx,
    );
    check("voicing.generate", &voicing);

    let arrangement = call_ok(
        &core,
        "arrangement.generate",
        json_obj! { "candidate_id" => candidate_id.clone() },
        &ctx,
    );
    check("arrangement.generate", &arrangement);

    for args in [
        json_obj! { "snapshot_id" => snapshot_id.clone() },
        json_obj! { "analysis_id" => analysis_id.clone() },
        json_obj! { "candidate_id" => candidate_id.clone() },
    ] {
        let audit = call_ok(&core, "loop.audit", args, &ctx);
        check("loop.audit", &audit);
    }

    for detail in ["concise", "detailed"] {
        let explained = call_ok(
            &core,
            "candidate.explain",
            json_obj! { "candidate_id" => candidate_id.clone(), "detail" => detail },
            &ctx,
        );
        check("candidate.explain", &explained);
    }
}

fn call_ok(core: &ServerCore, name: &str, args: Json, ctx: &CallContext) -> Json {
    let r = reaper_music_mcp::tools::call(core, name, &args, ctx);
    assert_eq!(
        r.get("isError"),
        Some(&Json::Bool(false)),
        "{name} failed: {:?}",
        r.get("structuredContent")
    );
    r.get("structuredContent").unwrap().clone()
}

#[test]
fn a_tool_result_always_carries_content_and_structured_content() {
    let mut h = Harness::started();
    for (name, args) in [
        ("reaper.status", json_obj! {}),
        ("theory.search", json_obj! { "query" => "dominant" }),
        ("candidate.explain", json_obj! { "candidate_id" => "nope" }),
    ] {
        let r = h.call_tool(name, args);
        let content = r.arr_field("content").expect("content");
        assert_eq!(content.len(), 1, "{name}");
        assert_eq!(content[0].str_field("type").unwrap(), "text");
        let text = content[0].str_field("text").unwrap();
        let parsed = Json::parse(text).expect("the text fallback must be the same JSON");
        assert_eq!(&parsed, r.get("structuredContent").unwrap(), "{name}");
    }
}

// ---------------------------------------------------------------------------
// §28: cancellation
// ---------------------------------------------------------------------------

#[test]
fn cancellation_does_not_corrupt_state() {
    let core = ServerCore::offline();
    let snapshot_id = seed_snapshot(&core, "melodies/eight_bar_c_major");

    // A generation that is cancelled before it starts must leave the store
    // exactly as it found it.
    let before = core.with_store(|s| s.stats());
    let cancelled = CallContext::detached();
    cancelled.cancel.cancel();
    let r = reaper_music_mcp::tools::call(
        &core,
        "harmony.generate_candidates",
        &json_obj! { "snapshot_id" => snapshot_id.clone(), "candidate_count" => 3 },
        &cancelled,
    );
    assert_eq!(r.get("isError"), Some(&Json::Bool(true)));
    assert_eq!(
        r.get("structuredContent")
            .unwrap()
            .str_field("error_code")
            .unwrap(),
        "CANCELLED"
    );
    let after = core.with_store(|s| s.stats());
    assert_eq!(before.to_canonical_string(), after.to_canonical_string());

    // And the server must still work afterwards: a cancellation is not a
    // poisoned session.
    let ok = reaper_music_mcp::tools::call(
        &core,
        "harmony.generate_candidates",
        &json_obj! { "snapshot_id" => snapshot_id, "candidate_count" => 2, "seed" => 3 },
        &CallContext::detached(),
    );
    assert_eq!(ok.get("isError"), Some(&Json::Bool(false)));
    assert_eq!(
        ok.get("structuredContent")
            .unwrap()
            .i64_field("candidate_count"),
        Ok(2)
    );
}

#[test]
fn a_cancellation_notification_is_accepted_and_answered_with_nothing() {
    let mut h = Harness::started();
    h.notify(
        "notifications/cancelled",
        json_obj! { "requestId" => 99, "reason" => "user pressed escape" },
    );
    // The session keeps working.
    let tools = h.result("tools/list", json_obj! {});
    assert_eq!(tools.arr_field("tools").unwrap().len(), 14);
}

#[test]
fn a_cancellation_that_arrives_before_its_request_is_still_honoured() {
    let mut h = Harness::started();
    let core = Arc::clone(&h.server.core);
    let snapshot_id = seed_snapshot(&core, "melodies/eight_bar_c_major");

    h.notify(
        "notifications/cancelled",
        json_obj! { "requestId" => h.next_id },
    );
    let r = h.request(
        "tools/call",
        json_obj! {
            "name" => "harmony.generate_candidates",
            "arguments" => json_obj! { "snapshot_id" => snapshot_id, "candidate_count" => 2 },
        },
    );
    let payload = r
        .get("result")
        .unwrap()
        .get("structuredContent")
        .unwrap()
        .clone();
    assert_eq!(payload.str_field("error_code").unwrap(), "CANCELLED");
}

// ---------------------------------------------------------------------------
// §28: expired ids
// ---------------------------------------------------------------------------

#[test]
fn expired_candidate_ids_are_rejected() {
    let core = ServerCore::offline();
    let snapshot_id = seed_snapshot(&core, "melodies/eight_bar_c_major");
    let generation = call_ok(
        &core,
        "harmony.generate_candidates",
        json_obj! { "snapshot_id" => snapshot_id, "candidate_count" => 1, "seed" => 8 },
        &CallContext::detached(),
    );
    let candidate_id = generation.arr_field("candidates").unwrap()[0]
        .str_field("candidate_id")
        .unwrap()
        .to_string();

    // Live: it works.
    let ok = reaper_music_mcp::tools::call(
        &core,
        "candidate.explain",
        &json_obj! { "candidate_id" => candidate_id.clone() },
        &CallContext::detached(),
    );
    assert_eq!(ok.get("isError"), Some(&Json::Bool(false)));

    // Past its TTL: refused with a clear code, not silently regenerated.
    let expired = CallContext {
        now: qjson::time::unix_now() + reaper_music_mcp::store::CANDIDATE_TTL_SECONDS + 1,
        ..CallContext::detached()
    };
    for tool in [
        "candidate.explain",
        "voicing.generate",
        "arrangement.generate",
        "reaper.stage_candidate",
    ] {
        let r = reaper_music_mcp::tools::call(
            &core,
            tool,
            &json_obj! { "candidate_id" => candidate_id.clone() },
            &expired,
        );
        assert_eq!(r.get("isError"), Some(&Json::Bool(true)), "{tool}");
        let code = r
            .get("structuredContent")
            .unwrap()
            .str_field("error_code")
            .unwrap()
            .to_string();
        assert_eq!(code, "EXPIRED_ID", "{tool}");
    }
}

#[test]
fn an_unknown_id_is_distinguished_from_an_expired_one() {
    let core = ServerCore::offline();
    let r = reaper_music_mcp::tools::call(
        &core,
        "candidate.explain",
        &json_obj! { "candidate_id" => "00000000-0000-4000-8000-000000000000" },
        &CallContext::detached(),
    );
    assert_eq!(
        r.get("structuredContent")
            .unwrap()
            .str_field("error_code")
            .unwrap(),
        "UNKNOWN_ID"
    );
}

#[test]
fn an_expired_analysis_id_is_rejected() {
    let core = ServerCore::offline();
    let snapshot_id = seed_snapshot(&core, "melodies/eight_bar_c_major");
    let analysis = call_ok(
        &core,
        "music.analyze_selection",
        json_obj! { "snapshot_id" => snapshot_id },
        &CallContext::detached(),
    );
    let expired = CallContext {
        now: qjson::time::unix_now() + reaper_music_mcp::store::ANALYSIS_TTL_SECONDS + 1,
        ..CallContext::detached()
    };
    let r = reaper_music_mcp::tools::call(
        &core,
        "loop.audit",
        &json_obj! { "analysis_id" => analysis.str_field("analysis_id").unwrap() },
        &expired,
    );
    assert_eq!(
        r.get("structuredContent")
            .unwrap()
            .str_field("error_code")
            .unwrap(),
        "EXPIRED_ID"
    );
}

// ---------------------------------------------------------------------------
// Transport-level edge cases
// ---------------------------------------------------------------------------

#[test]
fn a_malformed_envelope_gets_a_json_rpc_error_with_the_id_preserved() {
    let mut h = Harness::started();
    let response = h
        .server
        .handle(&json_obj! { "id" => 42, "method" => "ping" })
        .expect("a response");
    assert_eq!(response.get("id"), Some(&Json::Int(42)));
    assert_eq!(response.get("error").unwrap().i64_field("code"), Ok(-32600));
}

#[test]
fn an_unknown_method_is_method_not_found() {
    let mut h = Harness::started();
    let r = h.request("resources/subscribe", json_obj! {});
    assert_eq!(r.get("error").unwrap().i64_field("code"), Ok(-32601));
}

#[test]
fn tools_call_with_a_missing_name_is_invalid_params() {
    let mut h = Harness::started();
    let r = h.request("tools/call", json_obj! { "arguments" => json_obj! {} });
    assert_eq!(r.get("error").unwrap().i64_field("code"), Ok(-32602));
}

#[test]
fn logging_set_level_is_supported_as_declared() {
    let mut h = Harness::started();
    let caps = h
        .request(
            "initialize",
            json_obj! { "protocolVersion" => MCP_PROTOCOL_VERSION },
        )
        .get("result")
        .unwrap()
        .get("capabilities")
        .unwrap()
        .clone();
    assert!(caps.get("logging").is_some());
    let before = reaper_music_mcp::log::level();
    assert!(h
        .request("logging/setLevel", json_obj! { "level" => "warning" })
        .get("result")
        .is_some());
    reaper_music_mcp::log::set_level(before);
}
